# Debug Brief: Pi-Grok × Non-Compact × Expanded Edit = Lag

## Confirmed Bug Matrix

| Scenario | Compact ON | Compact OFF |
|---|---|---|
| **Pi-Grok** live turn + expanded Edit | ✅ Smooth | ❌ **LAG** |
| **Pi-Grok** resume/idle + expanded Edit | ✅ Smooth | ❌ **LAG** |
| **Upstream Grok** (any state, any content) | ✅ Smooth | ✅ Smooth |

## Key Constraints

1. **Resume proves it's NOT event-driven.** No ACP messages, no spinner tick, no live-turn loop. The lag persists with zero external events.
2. **Upstream Grok proves it's NOT the Pager render engine alone.** Same binary, same render code, same compact OFF + sticky headers + expanded Edit → smooth.
3. **Compact toggle is an instant fix.** Same session, same scroll position, toggle compact → immediately smooth. Toggle back → immediately laggy.
4. **The lag is per-frame.** It's not a one-time cost (expand animation). Every scroll tick, every key press, every redraw stutters.

## What Compact Mode Changes (the ONLY differences)

```rust
// crates/codegen/xai-grok-pager-render/src/appearance/config.rs
eff_outer_vpad:  1 → 0   (scrollback area gains 2 rows)
eff_hpad_left:   2 → 1   (scrollback area gains 1 col)
eff_hpad_right:  2 → 1   (scrollback area gains 1 col)
sticky_headers:  ON → OFF (render path skips sticky entirely)
prompt.compact:  false → true (prompt chrome simplified)
```

**Entry content width** (the width used for block.output() and caching) is determined by `HorizontalLayout` using `block_pad_left/right` (always 2/2) — **NOT affected by compact mode**. The 2-col outer hpad difference changes the scrollback viewport width passed to `prepare_layout`, which changes `entry_area_width` by 2 cols, which changes `content_width` by 2 cols.

## Render Path (non-compact, steady-state frame)

```
AgentView::draw()
  → scrollback.set_cwd(...)          // no-op if unchanged
  → scrollback.prepare_layout(w, h)  // Case 3: compute_total_height + settle_visible_measurements
  → ScrollbackPane::render_with_scratch_and_selection_boundaries()
    → render_with_sticky_headers()
      → build_prompt_descriptors_for_range()  // O(prompts) from cache
      → compute_sticky_layout()               // O(prompts), cheap
      → [if pinned] render_sticky_header()
        → render_entry_with_ctx_static()
          → entry.block.output(ctx)           // ⚠️ UNCACHED — full re-render every frame
      → render_content()
        → render_scrolled_entries_with_selection_boundaries()
          → per visible entry:
            → EntryRenderer::render()
              → entry.ensure_cached(width, ...) // CACHED (hit after first frame)
            → entry.ensure_cached(...) again for selection model
```

## Hypothesis 1 (HIGHEST PRIORITY): Sticky Header Uncached Render

**File:** `crates/codegen/xai-grok-pager/src/scrollback/scrollback_pane.rs:730`

```rust
fn render_entry_with_ctx_static(...) {
    let output = entry.block.output(ctx);  // ← FULL RE-RENDER, NO CACHE
    ...
}
```

This is called every frame for the pinned/pushed sticky header. It bypasses `ensure_cached()` entirely. For a UserPromptBlock with markdown, this means re-parsing markdown + word-wrap every frame.

**Why Pi-specific:** Pi user prompts may be longer/more complex (multi-paragraph instructions with code blocks). Upstream Grok prompts might be shorter.

**Why compact fixes it:** Compact mode disables sticky headers entirely → this code path never executes.

**Test:** In `render_sticky_header`, replace `entry.block.output(ctx)` with a cached lookup (call `entry.ensure_cached(...)` first, then use `entry.cached_output_ref()`). If lag disappears, this is the root cause.

**Counter-argument:** This would also affect upstream Grok with long prompts + non-compact. Unless Pi prompts are systematically longer, or the issue is specifically the Edit block being the pinned entry (but sticky only pins user prompts).

## Hypothesis 2: settle_visible_measurements Non-Convergence

**File:** `crates/codegen/xai-grok-pager/src/scrollback/state/layout.rs:697`

```rust
pub(super) fn settle_visible_measurements(&mut self, width: u16) {
    let max_iters = self.entries.len().saturating_add(2);
    for _ in 0..max_iters {
        let Some((start, end)) = self.measurement_window() else { return; };
        if !self.measure_window_exact(width, start, end) { return; }
        self.rebuild_virtual_y_from_heights();
        self.compute_total_height_from_cache();
        if self.follow_mode { self.follow_scroll_to_bottom(); } else { ... }
    }
}
```

Each iteration measures visible entries. If an exact height differs from the estimate, virtual_y shifts, revealing new entries at the viewport edge, which need measurement, which shifts again...

**Why Pi-specific:** Pi's Edit blocks (from `write` tool → full file content as diff) can be VERY tall (hundreds of lines). The estimate (`estimate_content_lines` using `searchable_text()`) might be wildly wrong for Edit blocks (it counts raw text lines, not syntax-highlighted wrapped diff lines). This causes large estimate→exact deltas, triggering many iterations.

**Why compact fixes it:** With 2 extra rows of viewport height, the measurement window is slightly larger, potentially converging in fewer iterations. OR: the 2-col width difference changes wrap points just enough to avoid the cascade.

**Test:** Add a counter to `settle_visible_measurements` and log iteration count per frame. If it's >2 consistently in non-compact Pi sessions, this is the cause.

## Hypothesis 3: Edit Block estimate_height is Pathologically Wrong

**File:** `crates/codegen/xai-grok-pager/src/scrollback/wrappers/entry_renderer.rs:431`

```rust
fn estimate_content_lines(&self, content_width: u16) -> u16 {
    ...
    match self.entry.block.searchable_text() {
        Some(text) => estimate_wrapped_line_count(&text, content_width),
        None => 1,
    }
}
```

For EditToolCallBlock, `searchable_text()` returns the raw diff text. But the RENDERED height includes:
- Gutter (line numbers): ~6-8 chars per line
- Indent: 2 chars
- Content gap: 2 chars
- Syntax highlighting doesn't change line count, but the gutter reduces effective content width

So the estimate uses full `content_width` for wrapping, but the actual render uses `content_width - gutter_width`. This means the estimate UNDERESTIMATES height for Edit blocks.

**Why this matters:** If the estimate is 50 lines but exact is 80 lines, the virtual_y shift is 30 rows, which reveals 30 new rows of entries below, which need measurement, which might reveal more...

**Why compact fixes it:** The 2-col width difference might coincidentally reduce the estimate error, or the larger viewport absorbs the shift without revealing new unmeasured entries.

**Test:** In `measure_window_exact`, log `|exact - estimate|` for Edit blocks. If consistently >20% off, fix the estimate to account for gutter width.

## Hypothesis 4: Pi Edit Blocks Have Enormous Hunks

Pi's `write` tool creates an Edit with `old_text: None, new_text: <entire file>`. The resulting diff is a single hunk with the entire file as insertions. For a 500-line file, the expanded Edit block is 500+ rendered lines.

Combined with Hypothesis 2/3, this makes the measurement cascade much worse than upstream Grok's typical small edits.

**Test:** Check the hunk sizes in a Pi session's Edit blocks. If they're 100+ lines, this amplifies H2/H3.

## Hypothesis 5: Width-Dependent Cache Thrashing

The 2-col width difference between compact/non-compact means:
- Non-compact: content_width = W
- Compact: content_width = W+2

If any code path alternates between these widths (e.g., sticky header uses one width, content render uses another), the `CachedOutput` thrashes every frame.

**Check:** `render_sticky_header` uses `layout.content_width()` from the HEADER area (full viewport width). Content render uses `entry_row_layout.content_width()` from the CONTENT area (below header). If the header takes some width... no, headers are full-width. But the content area's `HorizontalLayout` is created from `content_area` which is the same width as the full area. So widths should match.

## Already Ruled Out

- ❌ ACP event frequency (resume has no events)
- ❌ Live-turn tick/spinner (resume is idle)
- ❌ `has_expanded_edit_in_viewport` throttle (only affects live turn)
- ❌ `suppress_sticky_headers` (only affects live turn)
- ❌ SPINNER_DIVISOR change (irrelevant to resume)
- ❌ Terminal output volume (resume has no output)
- ❌ The render engine itself (upstream Grok is fine)

## Suggested Fix Priority

1. **Cache the sticky header render** (H1): Use `ensure_cached()` in `render_sticky_header` instead of raw `block.output()`. This is a clear correctness bug regardless of whether it's THE cause.

2. **Fix Edit block height estimate** (H3): Account for gutter width in `estimate_content_lines` for Edit blocks. This prevents measurement cascades.

3. **Cap settle_visible_measurements iterations** (H2): If iterations > 3, accept the current state and mark remaining entries for lazy measurement.

4. **If none of the above fixes it:** Add `tracing::info!` timing around `prepare_layout`, `render_with_sticky_headers`, and `render_content` in a debug build. Have the user reproduce with `RUST_LOG=xai_grok_pager=info` and capture the per-frame breakdown.

## File Map

| File | Role |
|---|---|
| `crates/codegen/xai-grok-pager/src/scrollback/scrollback_pane.rs` | Sticky header render (H1) |
| `crates/codegen/xai-grok-pager/src/scrollback/state/layout.rs` | prepare_layout, settle_visible_measurements (H2) |
| `crates/codegen/xai-grok-pager/src/scrollback/wrappers/entry_renderer.rs` | estimate_content_lines (H3) |
| `crates/codegen/xai-grok-pager/src/scrollback/blocks/tool/edit.rs` | EditToolCallBlock::rendered_output |
| `crates/codegen/xai-grok-pager/src/scrollback/entry.rs` | CachedOutput, ensure_cached |
| `crates/codegen/xai-grok-pager/src/scrollback/render.rs` | render_scrolled_entries (per-frame entry render) |
| `crates/codegen/xai-grok-pager/src/scrollback/state/mod.rs` | prepare_layout entry point, tick |
| `crates/codegen/xai-grok-pager/src/app/agent_view/render.rs` | AgentView::draw (calls prepare_layout + render) |
| `crates/codegen/pi-grok-adapter/src/tool_projection.rs` | edit_diff_content (Pi→ACP diff creation) |
| `crates/codegen/xai-grok-pager-render/src/appearance/config.rs` | LayoutConfig, compact mode differences |

## Reproduction

1. Start `grok-pi` (Pi integration mode)
2. Ask Pi to write/edit a file (creates an Edit block)
3. Expand the Edit block (Enter on it)
4. Toggle compact mode OFF (`/compact-mode`)
5. Scroll or press any key → observe per-frame stutter
6. Toggle compact mode ON → immediately smooth
7. For resume variant: exit, resume the same session, expand Edit, observe lag with compact OFF
