//! Viewport-derived turn navigation for the timeline sidebar.

use super::*;

impl ScrollbackState {
    /// Preview text for one turn, used by the timeline rail hover card.
    pub fn turn_preview(&self, turn_idx: usize) -> Option<String> {
        let turn = self.turns.get(turn_idx)?;
        let entry = self.entries.get_index(turn.prompt_index)?.1;
        let RenderBlock::UserPrompt(prompt) = &entry.block else {
            return None;
        };
        let line = prompt
            .text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("");
        let mut preview: String = line.chars().take(120).collect();
        if line.chars().nth(120).is_some() {
            preview.pop();
            preview.push('…');
        }
        Some(preview)
    }

    /// The turn owning the viewport top, if any.
    pub fn active_turn_for_viewport(&self) -> Option<usize> {
        if self.view_mode == ViewMode::SingleTurn {
            return self.current_turn;
        }
        if self.turns.is_empty() {
            return None;
        }
        Some(self.prompts_above_top(false)?.saturating_sub(1))
    }

    /// Jump to a turn's prompt and anchor it at the viewport top.
    pub fn jump_to_turn(&mut self, turn_idx: usize) -> bool {
        if turn_idx >= self.turns.len() {
            return false;
        }
        self.activate_turn(turn_idx);
        true
    }

    /// The nearest turn strictly above the viewport top.
    pub fn turn_above_viewport_top(&self) -> Option<usize> {
        if self.view_mode == ViewMode::SingleTurn {
            return self.current_turn?.checked_sub(1);
        }
        self.prompts_above_top(true)?.checked_sub(1)
    }

    /// The nearest turn below the viewport top.
    pub fn turn_below_viewport_top(&self) -> Option<usize> {
        if self.view_mode == ViewMode::SingleTurn {
            let next = self.current_turn?.checked_add(1)?;
            return (next < self.turns.len()).then_some(next);
        }
        let next = self.prompts_above_top(false)?;
        (next < self.turns.len()).then_some(next)
    }

    fn prompts_above_top(&self, strict: bool) -> Option<usize> {
        let cache = self.layout_cache.as_ref()?;
        let range = self.visible_entry_range();
        let base = *cache.virtual_y.get(range.start)?;
        let top = base + self.scroll_offset;
        Some(self.turns.partition_point(|turn| {
            cache
                .virtual_y
                .get(turn.prompt_index)
                .is_some_and(|&prompt_y| {
                    if strict {
                        prompt_y < top
                    } else {
                        prompt_y <= top
                    }
                })
        }))
    }
}
