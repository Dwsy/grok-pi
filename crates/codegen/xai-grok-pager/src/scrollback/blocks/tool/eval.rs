//! EvalToolCallBlock - persistent-kernel code evaluation.

use ratatui::text::{Line, Span, Text};

use crate::appearance::AppearanceConfig;
use crate::render::wrapping::word_wrap_lines;
use crate::scrollback::block::BlockContent;
use crate::scrollback::types::{
    AccentStyle, BlockBackground, BlockContext, BlockLine, BlockOutput, DisplayMode,
};
use crate::theme::Theme;

#[derive(Debug, Clone)]
pub struct EvalToolCallBlock {
    pub language: String,
    pub code: String,
    pub title: Option<String>,
    pub output: Option<String>,
    pub error: Option<String>,
    pub started_at: Option<std::time::Instant>,
    pub elapsed_ms: Option<i64>,
}

impl EvalToolCallBlock {
    pub fn new(language: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            language: language.into(),
            code: code.into(),
            title: None,
            output: None,
            error: None,
            started_at: None,
            elapsed_ms: None,
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        let title = title.into();
        self.title = (!title.trim().is_empty()).then_some(title);
        self
    }

    pub fn with_output(mut self, output: impl Into<String>) -> Self {
        let output = output.into();
        self.output = (!output.is_empty()).then_some(output);
        self
    }

    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    pub fn is_success(&self) -> bool {
        self.error.is_none()
    }

    pub fn finish(&mut self) {
        if self.elapsed_ms.is_none()
            && let Some(start) = self.started_at
        {
            self.elapsed_ms = Some(start.elapsed().as_millis() as i64);
        }
    }

    pub fn elapsed_ms(&self) -> Option<i64> {
        self.elapsed_ms.or_else(|| {
            self.started_at
                .map(|start| start.elapsed().as_millis() as i64)
        })
    }

    fn highlighted_code(&self, theme: &Theme) -> Vec<Line<'static>> {
        let syntect = crate::syntax::get_syntect();
        let mut highlighter = eval_syntax_token(&self.language)
            .and_then(|token| syntect.highlight_lines_for_token(token));
        let fallback = theme.fg(theme.md_code);

        self.code
            .lines()
            .map(|line| {
                Line::from(crate::syntax::highlight_line(
                    line,
                    &mut highlighter,
                    syntect,
                    fallback,
                ))
            })
            .collect()
    }

    fn header_line(&self, theme: &Theme, muted: bool) -> Line<'static> {
        let style = if muted {
            theme.muted()
        } else {
            theme.primary()
        };
        let bold = style.add_modifier(ratatui::style::Modifier::BOLD);
        let mut spans = vec![Span::styled("Eval".to_string(), bold)];
        let label = self
            .title
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| (!self.language.trim().is_empty()).then_some(self.language.as_str()));
        if let Some(label) = label {
            spans.push(Span::styled(format!("  {label}"), style));
        }
        Line::from(spans)
    }

    fn render_body(&self, ctx: &BlockContext, include_output: bool) -> BlockOutput {
        let theme = Theme::current();
        let width = ctx.content_width().max(20);
        let mut lines: Vec<BlockLine> = vec![self.header_line(&theme, false).into()];

        if !self.code.is_empty() {
            lines.push(Line::from("").into());
            for line in word_wrap_lines(self.highlighted_code(&theme), width) {
                lines.push(BlockLine::styled(line));
            }
        }

        if include_output {
            if let Some(output) = &self.output {
                lines.push(Line::from("").into());
                let output_lines: Vec<Line<'static>> = output
                    .lines()
                    .map(|line| Line::from(Span::styled(line.to_string(), theme.muted())))
                    .collect();
                for line in word_wrap_lines(output_lines, width) {
                    lines.push(BlockLine::styled(line));
                }
            }
            if let Some(error) = &self.error {
                lines.push(Line::from("").into());
                for line in error.lines() {
                    lines.push(BlockLine::styled(Line::from(Span::styled(
                        line.to_string(),
                        theme.fg(theme.accent_error),
                    ))));
                }
            }
        }

        BlockOutput { lines }
    }
}

fn eval_syntax_token(language: &str) -> Option<&'static str> {
    let language = language.trim();
    if language.eq_ignore_ascii_case("py") || language.eq_ignore_ascii_case("python") {
        return Some("python");
    }
    if language.eq_ignore_ascii_case("js")
        || language.eq_ignore_ascii_case("javascript")
        || language.eq_ignore_ascii_case("node")
        || language.eq_ignore_ascii_case("nodejs")
    {
        return Some("javascript");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> BlockContext {
        BlockContext {
            width: 80,
            mode: DisplayMode::Expanded,
            is_running: false,
            raw: false,
            max_lines: None,
            appearance: Default::default(),
            is_selected: false,
            cwd: None,
        }
    }

    #[test]
    fn expanded_code_preserves_python_syntax_token_spans() {
        let block = EvalToolCallBlock::new("py", "def add(x):\n    return x + 1");
        let output = block.output(&ctx());
        let code_spans: usize = output.lines[2..4]
            .iter()
            .map(|line| line.content.spans.len())
            .sum();
        assert!(
            code_spans > 2,
            "Python code should contain multiple syntax spans, got {code_spans}"
        );
    }

    #[test]
    fn expanded_code_preserves_javascript_syntax_token_spans() {
        let block = EvalToolCallBlock::new(
            "js",
            "const values = [1, 2, 3];\nvalues.reduce((a, b) => a + b, 0);",
        );
        let output = block.output(&ctx());
        let code_spans: usize = output.lines[2..4]
            .iter()
            .map(|line| line.content.spans.len())
            .sum();
        assert!(
            code_spans > 2,
            "JavaScript code should contain multiple syntax spans, got {code_spans}"
        );
    }
}

impl BlockContent for EvalToolCallBlock {
    fn output(&self, ctx: &BlockContext) -> BlockOutput {
        let theme = Theme::current();
        match ctx.mode {
            DisplayMode::Collapsed => BlockOutput {
                lines: vec![
                    self.header_line(
                        &theme,
                        ctx.mute_when_collapsed(
                            ctx.appearance.scrollback.blocks.tool.muted_collapsed,
                        ),
                    )
                    .into(),
                ],
            },
            DisplayMode::Truncated | DisplayMode::Expanded => self.render_body(ctx, true),
        }
    }

    fn accent(&self, ctx: &BlockContext) -> Option<AccentStyle> {
        if ctx.mode == DisplayMode::Collapsed {
            return None;
        }
        let theme = Theme::current();
        if self.error.is_some() {
            Some(AccentStyle::static_color(theme.accent_error))
        } else if ctx.is_running {
            Some(AccentStyle::animated(theme.accent_running))
        } else {
            Some(AccentStyle::static_color(theme.accent_tool))
        }
    }

    fn bullet(&self, ctx: &BlockContext) -> Option<AccentStyle> {
        if self.error.is_some() {
            Some(AccentStyle::static_color(Theme::current().accent_error))
        } else if ctx.mode == DisplayMode::Collapsed {
            None
        } else {
            self.accent(ctx)
        }
    }

    fn has_vpad_for(&self, _appearance: &AppearanceConfig) -> bool {
        false
    }

    fn background(&self, _ctx: &BlockContext) -> BlockBackground {
        BlockBackground::None
    }

    fn is_foldable(&self) -> bool {
        !self.code.is_empty() || self.output.is_some() || self.error.is_some()
    }

    fn default_display_mode(&self) -> DisplayMode {
        DisplayMode::Collapsed
    }

    fn next_fold_mode(&self, current: DisplayMode, _is_running: bool) -> DisplayMode {
        match current {
            DisplayMode::Collapsed => DisplayMode::Expanded,
            DisplayMode::Truncated | DisplayMode::Expanded => DisplayMode::Collapsed,
        }
    }

    fn preamble(&self, _ctx: &BlockContext) -> Option<Text<'static>> {
        Some(Text::from(vec![self.header_line(&Theme::current(), false)]))
    }
}
