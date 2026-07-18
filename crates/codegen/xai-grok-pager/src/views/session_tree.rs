//! Pi session entry tree modal state and rendering.
//!
//! Aligns with Pi interactive `TreeSelector`:
//! - branch-compressed visual indent (single-child chains stay flat)
//! - `├─` / `└─` / `│` connectors + fold glyphs
//! - default filter hides settings + tool-only assistants
//! - TreeX-inspired sticky detail pane
//!
//! Navigation still goes through Pi `ctx.navigateTree` via the adapter bridge.

use crate::app::actions::{SessionTreeFilter, SessionTreeNode};
use crate::theme::Theme;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionTreeFocus {
    #[default]
    List,
    Search,
    LabelEdit,
    DetailExpanded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GutterInfo {
    /// Visual indent level where the connector lived (0-based).
    position: usize,
    /// true = draw `│`, false = draw space (ancestor was last sibling).
    show: bool,
}

/// One visible row after filter + fold + Pi visual recompute.
#[derive(Debug, Clone)]
struct VisibleRow {
    node_index: usize,
    indent: usize,
    show_connector: bool,
    is_last: bool,
    gutters: Vec<GutterInfo>,
    is_virtual_root_child: bool,
    multiple_roots: bool,
}

#[derive(Debug, Clone)]
pub struct SessionTreeState {
    pub nodes: Vec<SessionTreeNode>,
    pub leaf_id: Option<String>,
    pub filter: SessionTreeFilter,
    pub search_query: String,
    pub selected: usize,
    pub scroll: usize,
    pub folded: HashSet<String>,
    pub show_label_timestamps: bool,
    pub detail_expanded: bool,
    pub detail_scroll: usize,
    pub focus: SessionTreeFocus,
    pub label_draft: String,
    pub loading: bool,
    pub status: Option<String>,
    /// Absolute content rect from last render (for mouse hit-testing).
    pub content_rect: Option<Rect>,
    /// List area from last render.
    pub list_rect: Option<Rect>,
    /// First visible list row y (absolute).
    pub list_start_y: u16,
    /// Number of list rows rendered last frame (viewport height).
    pub list_viewport: usize,
}

impl SessionTreeState {
    pub fn loading() -> Self {
        Self {
            nodes: Vec::new(),
            leaf_id: None,
            filter: SessionTreeFilter::Default,
            search_query: String::new(),
            selected: 0,
            scroll: 0,
            folded: HashSet::new(),
            show_label_timestamps: false,
            detail_expanded: false,
            detail_scroll: 0,
            focus: SessionTreeFocus::List,
            label_draft: String::new(),
            loading: true,
            status: Some("Fetching Pi get_tree…".into()),
            content_rect: None,
            list_rect: None,
            list_start_y: 0,
            list_viewport: 0,
        }
    }

    pub fn with_nodes(nodes: Vec<SessionTreeNode>, leaf_id: Option<String>) -> Self {
        let mut state = Self::loading();
        state.loading = false;
        state.status = None;
        state.leaf_id = leaf_id.clone();
        state.nodes = nodes;
        // Prefer current leaf if visible after default filter.
        let selected = leaf_id
            .as_deref()
            .and_then(|leaf| {
                state
                    .visible_rows()
                    .into_iter()
                    .position(|row| state.nodes[row.node_index].id == leaf)
            })
            .unwrap_or(0);
        state.selected = selected;
        state.ensure_visible(12);
        state
    }

    pub fn replace_nodes(&mut self, nodes: Vec<SessionTreeNode>, leaf_id: Option<String>) {
        let prev_id = self.selected_id();
        self.nodes = nodes;
        self.leaf_id = leaf_id;
        self.loading = false;
        self.status = None;
        if let Some(id) = prev_id {
            if let Some(index) = self
                .visible_rows()
                .into_iter()
                .position(|row| self.nodes[row.node_index].id == id)
            {
                self.selected = index;
            } else {
                self.selected = 0;
            }
        } else {
            self.selected = 0;
        }
        self.clamp_selected();
        self.ensure_visible(12);
    }

    /// Pi TreeSelector filter + fold + visual recompute.
    fn visible_rows(&self) -> Vec<VisibleRow> {
        let search_tokens: Vec<String> = self
            .search_query
            .to_lowercase()
            .split_whitespace()
            .map(str::to_string)
            .collect();

        // 1) Filter by mode / search (structural order preserved).
        let mut filtered: Vec<usize> = Vec::new();
        for (index, node) in self.nodes.iter().enumerate() {
            // Pi: hide tool-only assistants unless current leaf.
            if node.entry_type == "message"
                && node.role == "assistant"
                && !node.has_text
                && !node.is_current
            {
                continue;
            }
            if !passes_filter(node, self.filter) {
                continue;
            }
            if !search_tokens.is_empty() {
                let hay = searchable_text(node).to_lowercase();
                if !search_tokens.iter().all(|token| hay.contains(token)) {
                    continue;
                }
            }
            filtered.push(index);
        }

        // 2) Fold: hide descendants of folded nodes (structural parent chain).
        if !self.folded.is_empty() {
            let mut skip: HashSet<&str> = HashSet::new();
            // Walk full structural order so parent-before-child holds.
            for node in &self.nodes {
                if let Some(parent) = node.parent_id.as_deref() {
                    if skip.contains(parent) || self.folded.contains(parent) {
                        skip.insert(node.id.as_str());
                    }
                }
            }
            filtered.retain(|&i| !skip.contains(self.nodes[i].id.as_str()));
        }

        if filtered.is_empty() {
            return Vec::new();
        }

        // 3) Visible parent map: nearest visible ancestor (Pi recalculateVisualStructure).
        let visible_ids: HashSet<&str> = filtered
            .iter()
            .map(|&i| self.nodes[i].id.as_str())
            .collect();
        let id_to_index: HashMap<&str, usize> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.as_str(), i))
            .collect();

        let find_visible_ancestor = |node_id: &str| -> Option<String> {
            let mut current = self
                .nodes
                .iter()
                .find(|n| n.id == node_id)
                .and_then(|n| n.parent_id.clone());
            while let Some(id) = current {
                if visible_ids.contains(id.as_str()) {
                    return Some(id);
                }
                current = id_to_index
                    .get(id.as_str())
                    .and_then(|&i| self.nodes[i].parent_id.clone());
            }
            None
        };

        // visible children in filtered order
        let mut visible_parent: HashMap<String, Option<String>> = HashMap::new();
        let mut visible_children: HashMap<Option<String>, Vec<String>> = HashMap::new();
        visible_children.insert(None, Vec::new());
        for &idx in &filtered {
            let id = self.nodes[idx].id.clone();
            let ancestor = find_visible_ancestor(&id);
            visible_parent.insert(id.clone(), ancestor.clone());
            visible_children.entry(ancestor).or_default().push(id);
        }

        let visible_roots = visible_children.get(&None).cloned().unwrap_or_default();
        let multiple_roots = visible_roots.len() > 1;

        // 4) DFS with Pi indent rules.
        // Stack: (id, indent, just_branched, show_connector, is_last, gutters, is_virtual_root_child)
        let mut stack: Vec<(String, usize, bool, bool, bool, Vec<GutterInfo>, bool)> = Vec::new();
        for (i, root_id) in visible_roots.iter().enumerate().rev() {
            let is_last = i == visible_roots.len() - 1;
            stack.push((
                root_id.clone(),
                if multiple_roots { 1 } else { 0 },
                multiple_roots,
                multiple_roots,
                is_last,
                Vec::new(),
                multiple_roots,
            ));
        }

        let filtered_set: HashMap<&str, usize> = filtered
            .iter()
            .map(|&i| (self.nodes[i].id.as_str(), i))
            .collect();

        let mut out = Vec::with_capacity(filtered.len());
        while let Some((id, indent, just_branched, show_connector, is_last, gutters, is_vrc)) =
            stack.pop()
        {
            let Some(&node_index) = filtered_set.get(id.as_str()) else {
                continue;
            };
            out.push(VisibleRow {
                node_index,
                indent,
                show_connector,
                is_last,
                gutters: gutters.clone(),
                is_virtual_root_child: is_vrc,
                multiple_roots,
            });

            let children = visible_children.get(&Some(id)).cloned().unwrap_or_default();
            let multiple_children = children.len() > 1;
            let child_indent = if multiple_children {
                indent + 1
            } else if just_branched && indent > 0 {
                indent + 1
            } else {
                indent
            };

            let connector_displayed = show_connector && !is_vrc;
            let current_display_indent = if multiple_roots {
                indent.saturating_sub(1)
            } else {
                indent
            };
            let connector_position = current_display_indent.saturating_sub(1);
            let child_gutters: Vec<GutterInfo> = if connector_displayed {
                let mut g = gutters;
                g.push(GutterInfo {
                    position: connector_position,
                    show: !is_last,
                });
                g
            } else {
                gutters
            };

            for (i, child_id) in children.iter().enumerate().rev() {
                let child_is_last = i == children.len() - 1;
                stack.push((
                    child_id.clone(),
                    child_indent,
                    multiple_children,
                    multiple_children,
                    child_is_last,
                    child_gutters.clone(),
                    false,
                ));
            }
        }
        out
    }

    /// Back-compat for callers that only need node indices.
    pub fn visible_indices(&self) -> Vec<usize> {
        self.visible_rows()
            .into_iter()
            .map(|row| row.node_index)
            .collect()
    }

    pub fn selected_node(&self) -> Option<&SessionTreeNode> {
        let rows = self.visible_rows();
        rows.get(self.selected)
            .map(|row| &self.nodes[row.node_index])
    }

    pub fn selected_id(&self) -> Option<String> {
        self.selected_node().map(|n| n.id.clone())
    }

    pub fn clamp_selected(&mut self) {
        let len = self.visible_rows().len();
        if len == 0 {
            self.selected = 0;
            return;
        }
        if self.selected >= len {
            self.selected = len - 1;
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        let len = self.visible_rows().len() as isize;
        if len == 0 {
            self.selected = 0;
            return;
        }
        let next = (self.selected as isize + delta).rem_euclid(len);
        self.selected = next as usize;
        self.detail_scroll = 0;
        self.ensure_visible(self.list_viewport.max(12));
    }

    pub fn page(&mut self, delta: isize, page: usize) {
        let len = self.visible_rows().len();
        if len == 0 {
            return;
        }
        if delta < 0 {
            self.selected = self.selected.saturating_sub(page.max(1));
        } else {
            self.selected = (self.selected + page.max(1)).min(len - 1);
        }
        self.detail_scroll = 0;
        self.ensure_visible(page.max(1));
    }

    pub fn ensure_visible(&mut self, viewport: usize) {
        let viewport = viewport.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + viewport {
            self.scroll = self.selected + 1 - viewport;
        }
    }

    pub fn cycle_filter_forward(&mut self) {
        self.filter = self.filter.cycle_forward();
        self.folded.clear();
        self.selected = 0;
        self.scroll = 0;
        self.clamp_selected();
    }

    pub fn cycle_filter_backward(&mut self) {
        self.filter = self.filter.cycle_backward();
        self.folded.clear();
        self.selected = 0;
        self.scroll = 0;
        self.clamp_selected();
    }

    pub fn set_filter(&mut self, filter: SessionTreeFilter) {
        self.filter = filter;
        self.folded.clear();
        self.selected = 0;
        self.scroll = 0;
        self.clamp_selected();
    }

    /// Foldable when at least one currently-visible row treats `entry_id`
    /// as its nearest visible ancestor (Pi TreeSelector semantics).
    pub fn is_foldable(&self, entry_id: &str) -> bool {
        let rows = self.visible_rows();
        let visible_ids: HashSet<&str> = rows
            .iter()
            .map(|r| self.nodes[r.node_index].id.as_str())
            .collect();
        let id_to_parent: HashMap<&str, Option<&str>> = self
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n.parent_id.as_deref()))
            .collect();
        for row in &rows {
            let n = &self.nodes[row.node_index];
            let mut cur = n.parent_id.as_deref();
            while let Some(pid) = cur {
                if visible_ids.contains(pid) {
                    if pid == entry_id {
                        return true;
                    }
                    break;
                }
                cur = id_to_parent.get(pid).copied().flatten();
            }
        }
        false
    }

    pub fn toggle_fold_selected(&mut self) -> bool {
        let Some(node) = self.selected_node().cloned() else {
            return false;
        };
        if !self.is_foldable(&node.id) && node.child_ids.is_empty() {
            return false;
        }
        if self.folded.contains(&node.id) {
            self.folded.remove(&node.id);
        } else {
            // Only fold if it has structural or visible children.
            if node.child_ids.is_empty() && !self.is_foldable(&node.id) {
                return false;
            }
            self.folded.insert(node.id);
        }
        self.clamp_selected();
        true
    }

    pub fn begin_label_edit(&mut self) {
        let label = self
            .selected_node()
            .and_then(|n| n.label.clone())
            .unwrap_or_default();
        self.label_draft = label;
        self.focus = SessionTreeFocus::LabelEdit;
    }

    pub fn clear_search_or_cancel_edit(&mut self) -> SessionTreeEsc {
        match self.focus {
            SessionTreeFocus::LabelEdit => {
                self.focus = SessionTreeFocus::List;
                self.label_draft.clear();
                SessionTreeEsc::Consumed
            }
            SessionTreeFocus::DetailExpanded => {
                self.detail_expanded = false;
                self.focus = SessionTreeFocus::List;
                SessionTreeEsc::Consumed
            }
            SessionTreeFocus::Search | SessionTreeFocus::List => {
                if !self.search_query.is_empty() {
                    self.search_query.clear();
                    self.folded.clear();
                    self.selected = 0;
                    self.scroll = 0;
                    SessionTreeEsc::Consumed
                } else {
                    SessionTreeEsc::Close
                }
            }
        }
    }

    /// Map absolute terminal (col,row) to a visible list index, if any.
    pub fn hit_test_list_row(&self, col: u16, row: u16) -> Option<usize> {
        let list = self.list_rect?;
        if col < list.x || col >= list.x.saturating_add(list.width) {
            return None;
        }
        if row < list.y || row >= list.y.saturating_add(list.height) {
            return None;
        }
        let rel = (row - list.y) as usize;
        let index = self.scroll + rel;
        let len = self.visible_rows().len();
        if index < len { Some(index) } else { None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTreeEsc {
    Consumed,
    Close,
}

fn passes_filter(node: &SessionTreeNode, filter: SessionTreeFilter) -> bool {
    let is_settings = matches!(
        node.entry_type.as_str(),
        "label" | "custom" | "model_change" | "thinking_level_change" | "session_info"
    );
    match filter {
        SessionTreeFilter::UserOnly => node.entry_type == "message" && node.role == "user",
        SessionTreeFilter::NoTools => {
            !is_settings && !(node.entry_type == "message" && node.role == "toolResult")
        }
        SessionTreeFilter::LabeledOnly => node.label.is_some(),
        SessionTreeFilter::All => true,
        SessionTreeFilter::Default => !is_settings,
    }
}

fn searchable_text(node: &SessionTreeNode) -> String {
    format!(
        "{} {} {} {} {}",
        node.role,
        node.preview,
        node.detail,
        node.label.as_deref().unwrap_or(""),
        node.entry_type
    )
}

fn format_ago(timestamp: Option<&str>) -> String {
    let Some(ts) = timestamp else {
        return String::new();
    };
    let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(ts) else {
        return String::new();
    };
    let now = chrono::Utc::now();
    let delta = now.signed_duration_since(parsed.with_timezone(&chrono::Utc));
    let secs = delta.num_seconds().max(0);
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

/// Build Pi-style prefix: gutters + connector/fold + path marker.
fn build_prefix(row: &VisibleRow, folded: bool, foldable: bool) -> String {
    let display_indent = if row.multiple_roots {
        row.indent.saturating_sub(1)
    } else {
        row.indent
    };
    let show_connector = row.show_connector && !row.is_virtual_root_child;
    let connector_position = if show_connector {
        display_indent.saturating_sub(1) as isize
    } else {
        -1
    };
    let total_chars = display_indent * 3;
    let mut prefix = String::with_capacity(total_chars + 4);
    for i in 0..total_chars {
        let level = i / 3;
        let pos_in_level = i % 3;
        if let Some(gutter) = row.gutters.iter().find(|g| g.position == level) {
            if pos_in_level == 0 {
                prefix.push(if gutter.show { '│' } else { ' ' });
            } else {
                prefix.push(' ');
            }
        } else if show_connector && level as isize == connector_position {
            match pos_in_level {
                0 => prefix.push(if row.is_last { '└' } else { '├' }),
                1 => prefix.push(if folded {
                    '⊞'
                } else if foldable {
                    '⊟'
                } else {
                    '─'
                }),
                _ => prefix.push(' '),
            }
        } else {
            prefix.push(' ');
        }
    }
    // Root fold marker when no connector
    if folded && !show_connector {
        prefix.push_str("⊞ ");
    }
    prefix
}

/// Render the session tree content into `area`.
///
/// Caller supplies ModalWindow chrome; do not draw a second titled border.
pub fn render_session_tree(
    buf: &mut Buffer,
    area: Rect,
    state: &mut SessionTreeState,
    theme: &Theme,
) {
    state.content_rect = Some(area);
    if area.width < 10 || area.height < 6 {
        state.list_rect = None;
        return;
    }
    buf.set_style(area, Style::default().bg(theme.bg_base));

    let detail_h = if state.detail_expanded {
        (area.height * 45 / 100).clamp(6, area.height.saturating_sub(6))
    } else {
        4.min(area.height.saturating_sub(4))
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // search
            Constraint::Min(3),    // list
            Constraint::Length(detail_h),
            Constraint::Length(1), // status/help
        ])
        .split(area);

    state.list_rect = Some(chunks[1]);
    state.list_start_y = chunks[1].y;
    state.list_viewport = chunks[1].height as usize;
    state.ensure_visible(state.list_viewport.max(1));

    // Search / loading
    let search = if state.loading {
        Line::from(Span::styled(
            format!(
                "  {} · filter [{}]",
                state.status.as_deref().unwrap_or("Loading tree from Pi…"),
                state.filter.label()
            ),
            Style::default().fg(theme.accent_user),
        ))
    } else if state.search_query.is_empty() {
        Line::from(vec![
            Span::styled(
                format!("  Type to search · [{}]: ", state.filter.label()),
                Style::default().fg(theme.text_secondary),
            ),
            Span::styled(
                if matches!(state.focus, SessionTreeFocus::Search) {
                    "▌"
                } else {
                    ""
                },
                Style::default().fg(theme.accent_user),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                format!("  Search · [{}]: ", state.filter.label()),
                Style::default().fg(theme.text_secondary),
            ),
            Span::styled(
                state.search_query.clone(),
                Style::default().fg(theme.accent_user),
            ),
            Span::styled("▌", Style::default().fg(theme.accent_user)),
        ])
    };
    Paragraph::new(search).render(chunks[0], buf);

    // List
    let rows = state.visible_rows();
    let list_h = chunks[1].height as usize;
    let start = state.scroll.min(rows.len().saturating_sub(1).max(0));
    let end = (start + list_h).min(rows.len());
    let mut lines: Vec<Line> = Vec::new();
    if state.loading {
        lines.push(Line::from(Span::styled(
            "  Waiting for Pi get_tree (large sessions can take a few seconds)…",
            Style::default().fg(theme.text_secondary),
        )));
    } else if rows.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (no entries match filter/search)",
            Style::default().fg(theme.text_secondary),
        )));
    } else {
        for (row_i, row) in rows[start..end].iter().enumerate() {
            let abs = start + row_i;
            let node = &state.nodes[row.node_index];
            let selected = abs == state.selected;
            let folded = state.folded.contains(&node.id);
            let foldable = !node.child_ids.is_empty() || state.is_foldable(&node.id);
            let prefix = build_prefix(row, folded, foldable);
            let path_marker = if node.is_current || node.on_active_path {
                "• "
            } else {
                "  "
            };
            let cursor = if selected { "› " } else { "  " };
            let label = node
                .label
                .as_deref()
                .map(|l| {
                    if state.show_label_timestamps {
                        if let Some(ts) = node.label_timestamp.as_deref() {
                            format!("[{l} {ts}] ")
                        } else {
                            format!("[{l}] ")
                        }
                    } else {
                        format!("[{l}] ")
                    }
                })
                .unwrap_or_default();

            // Pi colors: user accent, assistant success, tools muted.
            let (role_style, body_style) = role_styles(node, selected, theme);
            let preview = if node.preview.is_empty() {
                "(no content)"
            } else {
                node.preview.as_str()
            };
            // toolResult already embeds `[bash: …]` — don't prefix `toolResult:`.
            let content = if node.role == "toolResult"
                || node.role == "bashExecution"
                || node.preview.starts_with('[')
            {
                preview.to_string()
            } else if node.entry_type == "message" {
                format!("{}: {preview}", node.role)
            } else {
                preview.to_string()
            };

            let mut spans = vec![
                Span::styled(
                    cursor.to_string(),
                    if selected {
                        Style::default()
                            .fg(theme.accent_user)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    },
                ),
                Span::styled(prefix, Style::default().fg(theme.gray_dim)),
                Span::styled(
                    path_marker.to_string(),
                    if node.on_active_path || node.is_current {
                        Style::default().fg(theme.accent_user)
                    } else {
                        Style::default().fg(theme.gray_dim)
                    },
                ),
            ];
            if !label.is_empty() {
                spans.push(Span::styled(label, Style::default().fg(theme.warning)));
            }
            // Split "role: body" for colored role when present.
            if let Some((role_part, body_part)) = content.split_once(": ") {
                if matches!(node.role.as_str(), "user" | "assistant")
                    && node.entry_type == "message"
                {
                    spans.push(Span::styled(format!("{role_part}: "), role_style));
                    spans.push(Span::styled(body_part.to_string(), body_style));
                } else {
                    spans.push(Span::styled(content.clone(), body_style));
                }
            } else {
                spans.push(Span::styled(content, body_style));
            }
            if selected {
                // Highlight whole line background via style on first span width is hard;
                // keep bold accent cursor which is the Pi selection cue.
            }
            lines.push(Line::from(spans));
        }
    }
    Paragraph::new(lines).render(chunks[1], buf);

    render_detail(buf, chunks[2], state, theme);

    let count = rows.len();
    let pos = if count == 0 { 0 } else { state.selected + 1 };
    let help = if matches!(state.focus, SessionTreeFocus::LabelEdit) {
        format!("  label edit · Enter save · Esc cancel  ({pos}/{count})")
    } else if state.detail_expanded {
        format!(
            "  Ctrl+R collapse · ↑/↓ detail  ({pos}/{count}) [{}]",
            state.filter.label()
        )
    } else {
        format!(
            "  ({pos}/{count}) [{}]  ↑/↓ · click select · dblclick go · Tab fold · / search · o filter · Enter go · Esc",
            state.filter.label()
        )
    };
    Paragraph::new(Line::from(Span::styled(
        help,
        Style::default().fg(theme.gray_dim),
    )))
    .render(chunks[3], buf);
}

fn role_styles(node: &SessionTreeNode, selected: bool, theme: &Theme) -> (Style, Style) {
    let bold = if selected {
        Modifier::BOLD
    } else {
        Modifier::empty()
    };
    let role = match node.role.as_str() {
        "user" => Style::default().fg(theme.accent_user).add_modifier(bold),
        "assistant" => Style::default()
            .fg(theme.accent_assistant)
            .add_modifier(bold),
        _ => Style::default().fg(theme.text_secondary).add_modifier(bold),
    };
    let body = if selected {
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD)
    } else if node.on_active_path {
        Style::default().fg(theme.text_primary)
    } else if node.role == "toolResult" || node.role == "bashExecution" {
        Style::default().fg(theme.text_secondary)
    } else {
        Style::default().fg(theme.text_primary)
    };
    (role, body)
}

fn render_detail(buf: &mut Buffer, area: Rect, state: &SessionTreeState, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme.selection_border));
    let inner = block.inner(area);
    block.render(area, buf);
    let Some(node) = state.selected_node() else {
        Paragraph::new(Span::styled(
            "  No selection",
            Style::default().fg(theme.text_secondary),
        ))
        .render(inner, buf);
        return;
    };
    let ago = format_ago(node.timestamp.as_deref());
    let current = if node.is_current {
        "CURRENT"
    } else if let Some(leaf) = state.leaf_id.as_deref() {
        let leaf_i = state.nodes.iter().position(|n| n.id == leaf);
        let cur_i = state.nodes.iter().position(|n| n.id == node.id);
        match (leaf_i, cur_i) {
            (Some(l), Some(c)) if c < l => "↑ CURRENT",
            (Some(l), Some(c)) if c > l => "↓ CURRENT",
            _ => "away",
        }
    } else {
        ""
    };
    let meta = format!(
        "  depth {} · {} · {}{} · {}",
        node.depth,
        node.entry_type,
        if ago.is_empty() { "—" } else { &ago },
        node.label
            .as_ref()
            .map(|l| format!(" · [{l}]"))
            .unwrap_or_default(),
        current
    );
    let body = if node.detail.is_empty() {
        node.preview.clone()
    } else {
        node.detail.clone()
    };
    let mut lines = vec![Line::from(Span::styled(
        meta,
        Style::default().fg(theme.text_secondary),
    ))];
    if matches!(state.focus, SessionTreeFocus::LabelEdit) {
        lines.push(Line::from(vec![
            Span::styled("  label: ", Style::default().fg(theme.accent_user)),
            Span::raw(state.label_draft.clone()),
            Span::styled("▌", Style::default().fg(theme.accent_user)),
        ]));
    }
    let max_body = if state.detail_expanded {
        inner.height.saturating_sub(2) as usize
    } else {
        2
    };
    let body_lines: Vec<&str> = body.lines().collect();
    let start = state.detail_scroll.min(body_lines.len().saturating_sub(1));
    let slice = &body_lines[start..(start + max_body).min(body_lines.len())];
    for line in slice {
        lines.push(Line::from(Span::raw(format!("  {line}"))));
    }
    if !state.detail_expanded && body_lines.len() > max_body {
        lines.push(Line::from(Span::styled(
            "  … Ctrl+R expand",
            Style::default().fg(theme.gray_dim),
        )));
    }
    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .render(inner, buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(
        id: &str,
        parent: Option<&str>,
        depth: usize,
        role: &str,
        entry_type: &str,
        has_text: bool,
    ) -> SessionTreeNode {
        SessionTreeNode {
            id: id.into(),
            parent_id: parent.map(str::to_string),
            depth,
            is_leaf: true,
            is_current: false,
            on_active_path: false,
            role: role.into(),
            preview: if role == "toolResult" {
                "[bash: echo]".into()
            } else {
                format!("{role} body")
            },
            detail: format!("{role} detail"),
            label: None,
            label_timestamp: None,
            entry_type: entry_type.into(),
            timestamp: None,
            child_ids: Vec::new(),
            has_text,
        }
    }

    #[test]
    fn user_only_filter_hides_tools_and_settings() {
        let mut state = SessionTreeState::with_nodes(
            vec![
                node("u", None, 0, "user", "message", true),
                node("t", Some("u"), 1, "toolResult", "message", false),
                node("m", Some("u"), 1, "model", "model_change", false),
            ],
            Some("u".into()),
        );
        state.set_filter(SessionTreeFilter::UserOnly);
        let vis = state.visible_indices();
        assert_eq!(vis.len(), 1);
        assert_eq!(state.nodes[vis[0]].id, "u");
    }

    #[test]
    fn default_hides_tool_only_assistant_but_keeps_tools() {
        let mut a = node("a", Some("u"), 1, "assistant", "message", false);
        a.child_ids = vec!["t".into()];
        a.is_leaf = false;
        let state = SessionTreeState::with_nodes(
            vec![
                {
                    let mut u = node("u", None, 0, "user", "message", true);
                    u.child_ids = vec!["a".into()];
                    u.is_leaf = false;
                    u
                },
                a,
                node("t", Some("a"), 2, "toolResult", "message", false),
            ],
            Some("t".into()),
        );
        let ids: Vec<_> = state
            .visible_indices()
            .into_iter()
            .map(|i| state.nodes[i].id.as_str())
            .collect();
        // tool-only assistant hidden; user + tool visible
        assert!(ids.contains(&"u"));
        assert!(ids.contains(&"t"));
        assert!(!ids.contains(&"a"));
    }

    #[test]
    fn single_child_chain_stays_flat_visually() {
        // u -> a -> t  all single children → visual indent 0 for all when one root
        let mut u = node("u", None, 0, "user", "message", true);
        u.child_ids = vec!["a".into()];
        u.is_leaf = false;
        let mut a = node("a", Some("u"), 1, "assistant", "message", true);
        a.child_ids = vec!["t".into()];
        a.is_leaf = false;
        let t = node("t", Some("a"), 2, "toolResult", "message", false);
        let state = SessionTreeState::with_nodes(vec![u, a, t], Some("t".into()));
        let rows = state.visible_rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].indent, 0);
        assert_eq!(rows[1].indent, 0);
        assert_eq!(rows[2].indent, 0);
        assert!(!rows[1].show_connector);
    }

    #[test]
    fn branch_increases_indent() {
        // u with two children a1, a2
        let mut u = node("u", None, 0, "user", "message", true);
        u.child_ids = vec!["a1".into(), "a2".into()];
        u.is_leaf = false;
        let a1 = node("a1", Some("u"), 1, "assistant", "message", true);
        let a2 = node("a2", Some("u"), 1, "assistant", "message", true);
        let state = SessionTreeState::with_nodes(vec![u, a1, a2], Some("a1".into()));
        let rows = state.visible_rows();
        assert_eq!(rows[0].indent, 0);
        assert_eq!(rows[1].indent, 1);
        assert_eq!(rows[2].indent, 1);
        assert!(rows[1].show_connector);
        assert!(rows[2].show_connector);
    }

    #[test]
    fn fold_hides_descendants() {
        let mut parent = node("p", None, 0, "user", "message", true);
        parent.child_ids = vec!["c".into()];
        parent.is_leaf = false;
        let child = node("c", Some("p"), 1, "assistant", "message", true);
        let mut state = SessionTreeState::with_nodes(vec![parent, child], Some("c".into()));
        assert_eq!(state.visible_indices().len(), 2);
        state.selected = 0;
        assert!(state.toggle_fold_selected());
        assert_eq!(state.visible_indices().len(), 1);
    }
}
