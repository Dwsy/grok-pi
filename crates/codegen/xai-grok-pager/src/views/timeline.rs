//! Timeline sidebar: a per-turn tick rail that replaces the scrollbar gutter.

use std::ops::Range;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;

use crate::theme::Theme;

pub const RAIL_WIDTH: u16 = 2;
pub const MIN_TERMINAL_WIDTH: u16 = 60;
pub const MIN_TURNS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineRail {
    pub rect: Rect,
    pub window: Range<usize>,
    pub ticks_y: u16,
    pub active: Option<usize>,
    pub up_target: Option<usize>,
    pub down_target: Option<usize>,
    pub up_y: u16,
    pub down_y: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineHit {
    Tick(usize),
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RailViewport {
    pub active: Option<usize>,
    pub up_target: Option<usize>,
    pub down_target: Option<usize>,
    pub at_bottom: bool,
}

pub fn rail_width(
    show_timeline: bool,
    is_subagent_view: bool,
    area_width: u16,
    turn_count: usize,
) -> u16 {
    if show_timeline
        && !is_subagent_view
        && area_width >= MIN_TERMINAL_WIDTH
        && turn_count >= MIN_TURNS
    {
        RAIL_WIDTH
    } else {
        0
    }
}

pub fn compute_rail(
    scrollback_area: Rect,
    rail_x: u16,
    turn_count: usize,
    viewport: RailViewport,
) -> Option<TimelineRail> {
    if turn_count < MIN_TURNS {
        return None;
    }
    let max_ticks = (scrollback_area.height as usize).checked_sub(2)?;
    if max_ticks == 0 {
        return None;
    }
    let window = if turn_count <= max_ticks {
        0..turn_count
    } else {
        let tail_start = turn_count - max_ticks;
        let start = if viewport.at_bottom {
            viewport
                .active
                .map_or(tail_start, |active| active.min(tail_start))
        } else {
            viewport
                .active
                .unwrap_or(turn_count - 1)
                .saturating_sub(max_ticks / 2)
                .min(tail_start)
        };
        start..start + max_ticks
    };
    let top = scrollback_area.y + ((scrollback_area.height as usize - window.len() - 2) / 2) as u16;
    let ticks_y = top + 1;
    Some(TimelineRail {
        rect: Rect::new(
            rail_x,
            scrollback_area.y,
            RAIL_WIDTH,
            scrollback_area.height,
        ),
        window: window.clone(),
        ticks_y,
        active: viewport.active,
        up_target: viewport.up_target,
        down_target: viewport.down_target,
        up_y: top,
        down_y: ticks_y + window.len() as u16,
    })
}

pub fn chevron_target(rail: &TimelineRail, hit: TimelineHit) -> Option<usize> {
    match hit {
        TimelineHit::Tick(turn_idx) => Some(turn_idx),
        TimelineHit::Up => rail.up_target,
        TimelineHit::Down => rail.down_target,
    }
}

impl TimelineRail {
    pub fn hit(&self, col: u16, row: u16) -> Option<TimelineHit> {
        if !self.rect.contains((col, row).into()) {
            return None;
        }
        if row == self.up_y {
            return Some(TimelineHit::Up);
        }
        if row == self.down_y {
            return Some(TimelineHit::Down);
        }
        (row >= self.ticks_y)
            .then(|| (row - self.ticks_y) as usize)
            .filter(|relative| *relative < self.window.len())
            .map(|relative| TimelineHit::Tick(self.window.start + relative))
    }
}

pub fn render_rail(
    buf: &mut Buffer,
    rail: &TimelineRail,
    hovered: Option<TimelineHit>,
    theme: &Theme,
) {
    let dim = Style::default().fg(theme.gray_dim);
    let normal = Style::default().fg(theme.gray);
    let bright = Style::default().fg(theme.text_primary);
    let up_enabled = rail.up_target.is_some();
    let down_enabled = rail.down_target.is_some();
    let up_style = if hovered == Some(TimelineHit::Up) && up_enabled {
        bright
    } else if up_enabled {
        normal
    } else {
        dim
    };
    let down_style = if hovered == Some(TimelineHit::Down) && down_enabled {
        bright
    } else if down_enabled {
        normal
    } else {
        dim
    };
    let x = rail.rect.x + RAIL_WIDTH - 1;
    buf.set_span(x, rail.up_y, &Span::styled("▲", up_style), 1);
    buf.set_span(x, rail.down_y, &Span::styled("▼", down_style), 1);
    for (row, turn_idx) in rail.window.clone().enumerate() {
        let style = if rail.active == Some(turn_idx) || hovered == Some(TimelineHit::Tick(turn_idx))
        {
            bright
        } else {
            dim
        };
        let tick = if rail.active == Some(turn_idx) {
            " ●"
        } else if hovered == Some(TimelineHit::Tick(turn_idx)) {
            " ○"
        } else {
            " ─"
        };
        buf.set_span(
            rail.rect.x,
            rail.ticks_y + row as u16,
            &Span::styled(tick, style),
            RAIL_WIDTH,
        );
    }
}
