//! Shared prompt-area list overlay with a cursor and scroll window.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

const MAX_ROWS: usize = 15;

pub struct ListOverlay {
    pub len: usize,
    pub selected: usize,
}

pub struct RowCtx {
    pub is_cursor: bool,
    pub row_bg: Color,
    pub content_width: u16,
}

/// Optional search/filter line rendered between the title and the list rows.
pub struct SearchLine<'a> {
    /// The current query text (empty = placeholder shown).
    pub query: &'a str,
    /// Placeholder shown when query is empty.
    pub placeholder: &'a str,
    /// Whether the search input is focused (cursor visible).
    pub focused: bool,
}

impl ListOverlay {
    pub fn height(&self, screen_h: u16) -> u16 {
        self.height_with_search(screen_h, false)
    }

    /// Height accounting for an optional search line.
    pub fn height_with_search(&self, screen_h: u16, has_search: bool) -> u16 {
        let rows = self.len.min(MAX_ROWS) as u16;
        let search_extra: u16 = if has_search { 1 } else { 0 };
        let height = 2 + search_extra + rows;
        let cap = (screen_h as u32 * 60 / 100).max(6) as u16;
        height.min(cap) + 1
    }

    fn visible_rows(area: Rect, has_search: bool) -> usize {
        let base: u16 = 3;
        let search_extra: u16 = if has_search { 1 } else { 0 };
        area.height.saturating_sub(base + search_extra) as usize
    }

    fn scroll_offset(&self, visible_rows: usize) -> usize {
        if visible_rows > 0 && self.selected >= visible_rows {
            self.selected - visible_rows + 1
        } else {
            0
        }
    }

    pub fn row_at(&self, area: Rect, col: u16, row: u16) -> Option<usize> {
        self.row_at_with_search(area, col, row, false)
    }

    /// Row hit-testing accounting for an optional search line.
    pub fn row_at_with_search(&self, area: Rect, col: u16, row: u16, has_search: bool) -> Option<usize> {
        if area.height == 0 || area.width < 10 {
            return None;
        }
        if col < area.x || col >= area.x + area.width {
            return None;
        }
        if row < area.y || row >= area.y + area.height {
            return None;
        }
        let search_extra: u16 = if has_search { 1 } else { 0 };
        let first = area.y + 2 + search_extra;
        if row < first {
            return None;
        }
        let visible_rows = Self::visible_rows(area, has_search);
        let relative = (row - first) as usize;
        if relative >= visible_rows {
            return None;
        }
        let index = self.scroll_offset(visible_rows) + relative;
        (index < self.len).then_some(index)
    }

    pub fn render(
        &self,
        buf: &mut Buffer,
        area: Rect,
        title: &str,
        focused: bool,
        row_line: impl FnMut(usize, &RowCtx) -> Line<'static>,
    ) {
        self.render_with_search(buf, area, title, focused, None, row_line);
    }

    /// Render with an optional search/filter line between title and rows.
    pub fn render_with_search(
        &self,
        buf: &mut Buffer,
        area: Rect,
        title: &str,
        focused: bool,
        search: Option<SearchLine<'_>>,
        mut row_line: impl FnMut(usize, &RowCtx) -> Line<'static>,
    ) {
        if area.height == 0 || area.width < 10 {
            return;
        }

        let theme = Theme::current();
        let background = theme.bg_light;
        buf.set_style(area, Style::default().bg(background));

        let accent_style = Style::default().fg(theme.accent_user);
        for row in area.y..area.y + area.height {
            if let Some(cell) = buf.cell_mut((area.x, row)) {
                cell.set_symbol(crate::glyphs::accent_bar());
                cell.set_style(accent_style);
            }
        }

        let content_x = area.x + 3;
        let content_width = area.width.saturating_sub(5);
        let title_style = Style::default()
            .fg(theme.accent_user)
            .add_modifier(Modifier::BOLD);
        let mut row = area.y + 1;
        buf.set_line(
            content_x,
            row,
            &Line::from(Span::styled(title.to_string(), title_style)),
            content_width,
        );
        row += 1;

        // Render search line if present.
        let has_search = search.is_some();
        if let Some(ref search) = search {
            let search_style = if search.query.is_empty() {
                Style::default().fg(theme.gray_dim)
            } else {
                Style::default().fg(theme.text_primary)
            };
            let display = if search.query.is_empty() {
                search.placeholder.to_string()
            } else {
                let cursor = if search.focused { "█" } else { "" };
                format!("{}{}", search.query, cursor)
            };
            let prefix = Span::styled("/ ", Style::default().fg(theme.accent_user));
            let text = Span::styled(display, search_style);
            buf.set_line(
                content_x,
                row,
                &Line::from(vec![prefix, text]),
                content_width,
            );
            row += 1;
        }

        let visible_rows = Self::visible_rows(area, has_search);
        let scroll_offset = self.scroll_offset(visible_rows);
        for index in (scroll_offset..self.len).take(visible_rows) {
            if row >= area.y + area.height {
                break;
            }
            let is_cursor = index == self.selected;
            let row_bg = if is_cursor && focused {
                theme.bg_visual
            } else {
                background
            };
            let row_rect = Rect {
                x: content_x.saturating_sub(1),
                y: row,
                width: content_width + 2,
                height: 1,
            };
            buf.set_style(row_rect, Style::default().bg(row_bg));
            let context = RowCtx {
                is_cursor,
                row_bg,
                content_width,
            };
            let line = row_line(index, &context);
            buf.set_line(content_x, row, &line, content_width);
            row += 1;
        }

        if !focused {
            crate::render::color::blend_area(buf, area, Some((background, 0.66)), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        }
    }

    #[test]
    fn row_at_maps_rows_and_rejects_chrome() {
        let list = ListOverlay {
            len: 3,
            selected: 0,
        };
        assert_eq!(list.row_at(area(), 5, 1), None);
        assert_eq!(list.row_at(area(), 5, 2), Some(0));
        assert_eq!(list.row_at(area(), 5, 4), Some(2));
        assert_eq!(list.row_at(area(), 5, 5), None);
    }

    #[test]
    fn row_at_respects_scroll_window() {
        let list = ListOverlay {
            len: 20,
            selected: 19,
        };
        assert_eq!(list.row_at(area(), 5, 2), Some(13));
        assert_eq!(list.row_at(area(), 5, 8), Some(19));
    }
}
