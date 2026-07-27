//! Native Pi model-management center.
//!
//! This is a Pager surface over Pi's `models.json`; it never owns model
//! runtime semantics. Saving returns a reload outcome so the host can invoke
//! Pi's official `ctx.reload()` bridge.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};

use crate::pi_model_config::{
    PiModelConfig, PiModelConfigSnapshot, PiModelCost, PiProviderConfig, validate_document,
};
use crate::theme::Theme;
use crate::views::modal_window::{
    ModalSizing, ModalWindowConfig, ModalWindowState, Shortcut, render_modal_window,
};

const SHORTCUTS: [Shortcut<'static>; 9] = [
    Shortcut {
        label: "Tab pane",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "/ search",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "n new",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "c clone",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "e edit",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "d delete",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "s save+reload",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "a activate",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "Esc close",
        clickable: false,
        id: 0,
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiModelsPane {
    Providers,
    Models,
    Details,
}

impl PiModelsPane {
    fn next(self) -> Self {
        match self {
            Self::Providers => Self::Models,
            Self::Models => Self::Details,
            Self::Details => Self::Providers,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Providers => Self::Details,
            Self::Models => Self::Providers,
            Self::Details => Self::Models,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PiModelsOutcome {
    Close,
    Changed,
    Reload,
    Activate(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoticeKind {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone)]
struct Notice {
    kind: NoticeKind,
    message: String,
}

#[derive(Debug, Clone)]
enum ConfirmAction {
    DeleteProvider(String),
    DeleteModel { provider: String, index: usize },
    Reload,
    Restore,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailField {
    ProviderName,
    BaseUrl,
    ProviderApi,
    ApiKey,
    AuthHeader,
    Headers,
    ModelId,
    ModelName,
    ModelApi,
    Reasoning,
    Input,
    ContextWindow,
    MaxTokens,
    CostInput,
    CostOutput,
    CostCacheRead,
    CostCacheWrite,
    ThinkingLevelMap,
}

impl DetailField {
    fn label(self) -> &'static str {
        match self {
            Self::ProviderName => "Provider name",
            Self::BaseUrl => "Base URL",
            Self::ProviderApi => "Provider API",
            Self::ApiKey => "API key / env",
            Self::AuthHeader => "Auth header",
            Self::Headers => "Headers (JSON)",
            Self::ModelId => "Model ID",
            Self::ModelName => "Display name",
            Self::ModelApi => "Model API",
            Self::Reasoning => "Reasoning",
            Self::Input => "Input modalities",
            Self::ContextWindow => "Context window",
            Self::MaxTokens => "Max output tokens",
            Self::CostInput => "Cost input / M",
            Self::CostOutput => "Cost output / M",
            Self::CostCacheRead => "Cost cache read / M",
            Self::CostCacheWrite => "Cost cache write / M",
            Self::ThinkingLevelMap => "Thinking map (JSON)",
        }
    }

    fn is_toggle(self) -> bool {
        matches!(self, Self::AuthHeader | Self::Reasoning)
    }
}

#[derive(Debug, Clone)]
struct FieldEditor {
    field: DetailField,
    buffer: String,
    cursor: usize,
}

impl FieldEditor {
    fn new(field: DetailField, value: String) -> Self {
        let cursor = value.chars().count();
        Self {
            field,
            buffer: value,
            cursor,
        }
    }

    fn insert(&mut self, text: &str) {
        let byte = char_to_byte(&self.buffer, self.cursor);
        self.buffer.insert_str(byte, text);
        self.cursor += text.chars().count();
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = char_to_byte(&self.buffer, self.cursor - 1);
        let end = char_to_byte(&self.buffer, self.cursor);
        self.buffer.replace_range(start..end, "");
        self.cursor -= 1;
    }

    fn delete(&mut self) {
        if self.cursor >= self.buffer.chars().count() {
            return;
        }
        let start = char_to_byte(&self.buffer, self.cursor);
        let end = char_to_byte(&self.buffer, self.cursor + 1);
        self.buffer.replace_range(start..end, "");
    }
}

#[derive(Debug)]
pub struct PiModelsModalState {
    pub window: ModalWindowState,
    pub snapshot: PiModelConfigSnapshot,
    current_model: Option<String>,
    focus: PiModelsPane,
    provider_selected: usize,
    model_selected: usize,
    detail_selected: usize,
    provider_scroll: usize,
    model_scroll: usize,
    detail_scroll: usize,
    search_query: String,
    search_active: bool,
    search_pane: PiModelsPane,
    editor: Option<FieldEditor>,
    confirm: Option<ConfirmAction>,
    notice: Option<Notice>,
    provider_rect: Option<Rect>,
    model_rect: Option<Rect>,
    detail_rect: Option<Rect>,
    provider_rows: Vec<(Rect, usize)>,
    model_rows: Vec<(Rect, usize)>,
    detail_rows: Vec<(Rect, usize)>,
}

impl PiModelsModalState {
    pub fn open(current_model: Option<String>) -> Result<Self> {
        Ok(Self::from_snapshot(
            PiModelConfigSnapshot::load()?,
            current_model,
        ))
    }

    pub fn from_snapshot(snapshot: PiModelConfigSnapshot, current_model: Option<String>) -> Self {
        Self {
            window: ModalWindowState::new(),
            snapshot,
            current_model,
            focus: PiModelsPane::Providers,
            provider_selected: 0,
            model_selected: 0,
            detail_selected: 0,
            provider_scroll: 0,
            model_scroll: 0,
            detail_scroll: 0,
            search_query: String::new(),
            search_active: false,
            search_pane: PiModelsPane::Providers,
            editor: None,
            confirm: None,
            notice: None,
            provider_rect: None,
            model_rect: None,
            detail_rect: None,
            provider_rows: Vec::new(),
            model_rows: Vec::new(),
            detail_rows: Vec::new(),
        }
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> PiModelsOutcome {
        if self.editor.is_some() {
            return self.handle_editor_key(key);
        }
        if self.confirm.is_some() {
            return self.handle_confirm_key(key);
        }
        if self.search_active {
            return self.handle_search_key(key);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            || key.modifiers.contains(KeyModifiers::ALT)
            || key.modifiers.contains(KeyModifiers::SUPER)
        {
            return PiModelsOutcome::Changed;
        }
        match key.code {
            KeyCode::Esc => {
                if self.snapshot.is_dirty() {
                    self.confirm = Some(ConfirmAction::Close);
                    self.notice_info("Unsaved changes: close without saving?");
                    PiModelsOutcome::Changed
                } else {
                    PiModelsOutcome::Close
                }
            }
            KeyCode::Tab => {
                self.focus = self.focus.next();
                self.clear_search();
                PiModelsOutcome::Changed
            }
            KeyCode::BackTab => {
                self.focus = self.focus.previous();
                self.clear_search();
                PiModelsOutcome::Changed
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.focus = self.focus.previous();
                self.clear_search();
                PiModelsOutcome::Changed
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.focus = self.focus.next();
                self.clear_search();
                PiModelsOutcome::Changed
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_selection(-1);
                PiModelsOutcome::Changed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_selection(1);
                PiModelsOutcome::Changed
            }
            KeyCode::Home => {
                self.set_selection_start();
                PiModelsOutcome::Changed
            }
            KeyCode::End => {
                self.set_selection_end();
                PiModelsOutcome::Changed
            }
            KeyCode::PageUp => {
                for _ in 0..8 {
                    self.move_selection(-1);
                }
                PiModelsOutcome::Changed
            }
            KeyCode::PageDown => {
                for _ in 0..8 {
                    self.move_selection(1);
                }
                PiModelsOutcome::Changed
            }
            KeyCode::Char('/') => {
                self.search_active = true;
                self.search_pane = self.focus;
                self.search_query.clear();
                PiModelsOutcome::Changed
            }
            KeyCode::Char('n') => self.add_selected_kind(),
            KeyCode::Char('c') => self.clone_selected_kind(),
            KeyCode::Char('e') => self.begin_edit_selected(),
            KeyCode::Char(' ') => self.toggle_selected(),
            KeyCode::Char('d') => self.arm_delete(),
            KeyCode::Char('s') => self.save(),
            KeyCode::Char('r') => {
                if self.snapshot.is_dirty() {
                    self.confirm = Some(ConfirmAction::Reload);
                    self.notice_info("Discard local changes and reload models.json?");
                    PiModelsOutcome::Changed
                } else {
                    self.reload_from_disk()
                }
            }
            KeyCode::Char('u') => {
                self.confirm = Some(ConfirmAction::Restore);
                self.notice_info("Restore the latest models.json backup?");
                PiModelsOutcome::Changed
            }
            KeyCode::Char('a') => self.activate_selected(),
            KeyCode::Enter => match self.focus {
                PiModelsPane::Providers => {
                    self.focus = PiModelsPane::Models;
                    PiModelsOutcome::Changed
                }
                PiModelsPane::Models => {
                    self.focus = PiModelsPane::Details;
                    PiModelsOutcome::Changed
                }
                PiModelsPane::Details => self.begin_edit_selected(),
            },
            _ => PiModelsOutcome::Changed,
        }
    }

    pub fn handle_paste(&mut self, text: &str) -> PiModelsOutcome {
        if let Some(editor) = self.editor.as_mut() {
            editor.insert(text);
            return PiModelsOutcome::Changed;
        }
        if self.search_active {
            self.search_query.push_str(text);
            self.clamp_selection();
            return PiModelsOutcome::Changed;
        }
        PiModelsOutcome::Changed
    }

    pub fn handle_mouse(&mut self, mouse: &MouseEvent) -> PiModelsOutcome {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.focus_for_point(mouse.column, mouse.row);
                self.move_selection(-3);
                PiModelsOutcome::Changed
            }
            MouseEventKind::ScrollDown => {
                self.focus_for_point(mouse.column, mouse.row);
                self.move_selection(3);
                PiModelsOutcome::Changed
            }
            MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                if let Some((_, index)) = self
                    .provider_rows
                    .iter()
                    .find(|(rect, _)| contains(*rect, mouse.column, mouse.row))
                {
                    self.focus = PiModelsPane::Providers;
                    self.provider_selected = *index;
                    self.model_selected = 0;
                    self.detail_selected = 0;
                    return PiModelsOutcome::Changed;
                }
                if let Some((_, index)) = self
                    .model_rows
                    .iter()
                    .find(|(rect, _)| contains(*rect, mouse.column, mouse.row))
                {
                    self.focus = PiModelsPane::Models;
                    self.model_selected = *index;
                    self.detail_selected = 0;
                    return PiModelsOutcome::Changed;
                }
                if let Some((_, index)) = self
                    .detail_rows
                    .iter()
                    .find(|(rect, _)| contains(*rect, mouse.column, mouse.row))
                {
                    self.focus = PiModelsPane::Details;
                    self.detail_selected = *index;
                    return PiModelsOutcome::Changed;
                }
                self.focus_for_point(mouse.column, mouse.row);
                PiModelsOutcome::Changed
            }
            _ => PiModelsOutcome::Changed,
        }
    }

    fn handle_editor_key(&mut self, key: &KeyEvent) -> PiModelsOutcome {
        let Some(editor) = self.editor.as_mut() else {
            return PiModelsOutcome::Changed;
        };
        match key.code {
            KeyCode::Esc => {
                self.editor = None;
                self.notice = None;
            }
            KeyCode::Enter => {
                let field = editor.field;
                let value = editor.buffer.clone();
                match self.commit_field(field, &value) {
                    Ok(()) => {
                        self.editor = None;
                        self.notice_success("Draft updated. Press s to save and hot reload.");
                    }
                    Err(error) => self.notice_error(format!("Invalid value: {error:#}")),
                }
            }
            KeyCode::Backspace => editor.backspace(),
            KeyCode::Delete => editor.delete(),
            KeyCode::Left => editor.cursor = editor.cursor.saturating_sub(1),
            KeyCode::Right => {
                editor.cursor = (editor.cursor + 1).min(editor.buffer.chars().count());
            }
            KeyCode::Home => editor.cursor = 0,
            KeyCode::End => editor.cursor = editor.buffer.chars().count(),
            KeyCode::Char(ch)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::SUPER) =>
            {
                editor.insert(&ch.to_string());
            }
            _ => {}
        }
        PiModelsOutcome::Changed
    }

    fn handle_confirm_key(&mut self, key: &KeyEvent) -> PiModelsOutcome {
        match key.code {
            KeyCode::Esc | KeyCode::Char('n') => {
                self.confirm = None;
                self.notice = None;
                PiModelsOutcome::Changed
            }
            KeyCode::Char('y') => {
                let action = self.confirm.take();
                match action {
                    Some(ConfirmAction::DeleteProvider(name)) => {
                        self.snapshot.document.providers.remove(&name);
                        self.provider_selected = self.provider_selected.saturating_sub(1);
                        self.model_selected = 0;
                        self.detail_selected = 0;
                        self.notice_success(format!("Removed provider '{name}' from the draft."));
                        PiModelsOutcome::Changed
                    }
                    Some(ConfirmAction::DeleteModel { provider, index }) => {
                        if let Some(config) = self.snapshot.document.providers.get_mut(&provider)
                            && index < config.models.len()
                        {
                            let removed = config.models.remove(index);
                            self.model_selected = self.model_selected.saturating_sub(1);
                            self.detail_selected = 0;
                            self.notice_success(format!(
                                "Removed model '{}' from the draft.",
                                removed.id
                            ));
                        }
                        PiModelsOutcome::Changed
                    }
                    Some(ConfirmAction::Reload) => self.reload_from_disk(),
                    Some(ConfirmAction::Restore) => self.restore_latest(),
                    Some(ConfirmAction::Close) => PiModelsOutcome::Close,
                    None => PiModelsOutcome::Changed,
                }
            }
            _ => PiModelsOutcome::Changed,
        }
    }

    fn handle_search_key(&mut self, key: &KeyEvent) -> PiModelsOutcome {
        match key.code {
            KeyCode::Esc => self.clear_search(),
            KeyCode::Enter => self.search_active = false,
            KeyCode::Backspace => {
                self.search_query.pop();
                self.clamp_selection();
            }
            KeyCode::Char(ch)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::SUPER) =>
            {
                self.search_query.push(ch);
                self.clamp_selection();
            }
            _ => {}
        }
        PiModelsOutcome::Changed
    }

    fn clear_search(&mut self) {
        self.search_active = false;
        self.search_query.clear();
    }

    fn provider_names(&self) -> Vec<String> {
        let query = (self.search_pane == PiModelsPane::Providers)
            .then(|| self.search_query.trim().to_ascii_lowercase())
            .unwrap_or_default();
        self.snapshot
            .document
            .providers
            .keys()
            .filter(|name| query.is_empty() || name.to_ascii_lowercase().contains(&query))
            .cloned()
            .collect()
    }

    fn selected_provider_name(&self) -> Option<String> {
        self.provider_names().get(self.provider_selected).cloned()
    }

    fn model_indexes(&self) -> Vec<usize> {
        let Some(provider_name) = self.selected_provider_name() else {
            return Vec::new();
        };
        let Some(provider) = self.snapshot.document.providers.get(&provider_name) else {
            return Vec::new();
        };
        let query = (self.search_pane == PiModelsPane::Models)
            .then(|| self.search_query.trim().to_ascii_lowercase())
            .unwrap_or_default();
        provider
            .models
            .iter()
            .enumerate()
            .filter(|(_, model)| {
                query.is_empty()
                    || model.id.to_ascii_lowercase().contains(&query)
                    || model.name.to_ascii_lowercase().contains(&query)
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn selected_model_index(&self) -> Option<usize> {
        self.model_indexes().get(self.model_selected).copied()
    }

    fn selected_model(&self) -> Option<&PiModelConfig> {
        let provider_name = self.selected_provider_name()?;
        let model_index = self.selected_model_index()?;
        self.snapshot
            .document
            .providers
            .get(&provider_name)?
            .models
            .get(model_index)
    }

    fn selected_provider(&self) -> Option<&PiProviderConfig> {
        self.snapshot
            .document
            .providers
            .get(&self.selected_provider_name()?)
    }

    fn detail_fields(&self) -> Vec<DetailField> {
        let mut fields = vec![
            DetailField::ProviderName,
            DetailField::BaseUrl,
            DetailField::ProviderApi,
            DetailField::ApiKey,
            DetailField::AuthHeader,
            DetailField::Headers,
        ];
        if self.selected_model().is_some() {
            fields.extend([
                DetailField::ModelId,
                DetailField::ModelName,
                DetailField::ModelApi,
                DetailField::Reasoning,
                DetailField::Input,
                DetailField::ContextWindow,
                DetailField::MaxTokens,
                DetailField::CostInput,
                DetailField::CostOutput,
                DetailField::CostCacheRead,
                DetailField::CostCacheWrite,
                DetailField::ThinkingLevelMap,
            ]);
        }
        let query = (self.search_pane == PiModelsPane::Details)
            .then(|| self.search_query.trim().to_ascii_lowercase())
            .unwrap_or_default();
        fields
            .into_iter()
            .filter(|field| query.is_empty() || field.label().to_ascii_lowercase().contains(&query))
            .collect()
    }

    fn selected_field(&self) -> Option<DetailField> {
        self.detail_fields().get(self.detail_selected).copied()
    }

    fn move_selection(&mut self, delta: isize) {
        let len = match self.focus {
            PiModelsPane::Providers => self.provider_names().len(),
            PiModelsPane::Models => self.model_indexes().len(),
            PiModelsPane::Details => self.detail_fields().len(),
        };
        if len == 0 {
            return;
        }
        let selected = match self.focus {
            PiModelsPane::Providers => &mut self.provider_selected,
            PiModelsPane::Models => &mut self.model_selected,
            PiModelsPane::Details => &mut self.detail_selected,
        };
        *selected = if delta < 0 {
            selected.saturating_sub(delta.unsigned_abs())
        } else {
            (*selected + delta as usize).min(len - 1)
        };
        if self.focus == PiModelsPane::Providers {
            self.model_selected = 0;
            self.detail_selected = 0;
        } else if self.focus == PiModelsPane::Models {
            self.detail_selected = 0;
        }
    }

    fn set_selection_start(&mut self) {
        match self.focus {
            PiModelsPane::Providers => self.provider_selected = 0,
            PiModelsPane::Models => self.model_selected = 0,
            PiModelsPane::Details => self.detail_selected = 0,
        }
    }

    fn set_selection_end(&mut self) {
        match self.focus {
            PiModelsPane::Providers => {
                self.provider_selected = self.provider_names().len().saturating_sub(1)
            }
            PiModelsPane::Models => {
                self.model_selected = self.model_indexes().len().saturating_sub(1)
            }
            PiModelsPane::Details => {
                self.detail_selected = self.detail_fields().len().saturating_sub(1)
            }
        }
    }

    fn clamp_selection(&mut self) {
        self.provider_selected = self
            .provider_selected
            .min(self.provider_names().len().saturating_sub(1));
        self.model_selected = self
            .model_selected
            .min(self.model_indexes().len().saturating_sub(1));
        self.detail_selected = self
            .detail_selected
            .min(self.detail_fields().len().saturating_sub(1));
    }

    fn add_selected_kind(&mut self) -> PiModelsOutcome {
        self.clear_search();
        match self.focus {
            PiModelsPane::Providers => {
                let name = unique_provider_name(&self.snapshot.document.providers, "provider");
                self.snapshot
                    .document
                    .providers
                    .insert(name.clone(), PiProviderConfig::default());
                self.provider_selected = self
                    .provider_names()
                    .iter()
                    .position(|candidate| candidate == &name)
                    .unwrap_or(0);
                self.model_selected = 0;
                self.detail_selected = 0;
                self.notice_success(format!("Added provider '{name}' to the draft."));
            }
            PiModelsPane::Models | PiModelsPane::Details => {
                let Some(provider_name) = self.selected_provider_name() else {
                    self.notice_error("Create a provider before adding models.");
                    return PiModelsOutcome::Changed;
                };
                let provider = self
                    .snapshot
                    .document
                    .providers
                    .get_mut(&provider_name)
                    .expect("selected provider exists");
                let id = unique_model_id(provider, "model");
                provider.models.push(PiModelConfig {
                    id: id.clone(),
                    name: id.clone(),
                    input: vec!["text".to_owned()],
                    context_window: Some(128_000),
                    max_tokens: Some(8_192),
                    ..PiModelConfig::default()
                });
                self.model_selected = provider.models.len() - 1;
                self.detail_selected = 0;
                self.focus = PiModelsPane::Details;
                self.notice_success(format!("Added model '{id}' to the draft."));
            }
        }
        PiModelsOutcome::Changed
    }

    fn clone_selected_kind(&mut self) -> PiModelsOutcome {
        self.clear_search();
        match self.focus {
            PiModelsPane::Providers => {
                let Some(source_name) = self.selected_provider_name() else {
                    return PiModelsOutcome::Changed;
                };
                let Some(source) = self.snapshot.document.providers.get(&source_name).cloned()
                else {
                    return PiModelsOutcome::Changed;
                };
                let name = unique_provider_name(
                    &self.snapshot.document.providers,
                    &format!("{source_name}-copy"),
                );
                self.snapshot
                    .document
                    .providers
                    .insert(name.clone(), source);
                self.provider_selected = self
                    .provider_names()
                    .iter()
                    .position(|candidate| candidate == &name)
                    .unwrap_or(0);
                self.notice_success(format!("Cloned provider as '{name}'."));
            }
            PiModelsPane::Models | PiModelsPane::Details => {
                let Some(provider_name) = self.selected_provider_name() else {
                    return PiModelsOutcome::Changed;
                };
                let Some(index) = self.selected_model_index() else {
                    return PiModelsOutcome::Changed;
                };
                let provider = self
                    .snapshot
                    .document
                    .providers
                    .get_mut(&provider_name)
                    .expect("selected provider exists");
                let Some(mut model) = provider.models.get(index).cloned() else {
                    return PiModelsOutcome::Changed;
                };
                model.id = unique_model_id(provider, &format!("{}-copy", model.id));
                model.name = format!("{} Copy", model.name);
                let id = model.id.clone();
                provider.models.push(model);
                self.model_selected = provider.models.len() - 1;
                self.focus = PiModelsPane::Details;
                self.notice_success(format!("Cloned model as '{id}'."));
            }
        }
        PiModelsOutcome::Changed
    }

    fn begin_edit_selected(&mut self) -> PiModelsOutcome {
        if self.focus != PiModelsPane::Details {
            self.focus = self.focus.next();
            return PiModelsOutcome::Changed;
        }
        let Some(field) = self.selected_field() else {
            return PiModelsOutcome::Changed;
        };
        if field.is_toggle() {
            return self.toggle_selected();
        }
        self.editor = Some(FieldEditor::new(field, self.field_raw_value(field)));
        self.notice_info(format!(
            "Editing {} · Enter apply · Esc cancel",
            field.label()
        ));
        PiModelsOutcome::Changed
    }

    fn toggle_selected(&mut self) -> PiModelsOutcome {
        let Some(field) = self.selected_field() else {
            return PiModelsOutcome::Changed;
        };
        let Some(provider_name) = self.selected_provider_name() else {
            return PiModelsOutcome::Changed;
        };
        match field {
            DetailField::AuthHeader => {
                if let Some(provider) = self.snapshot.document.providers.get_mut(&provider_name) {
                    provider.auth_header = cycle_optional_bool(provider.auth_header);
                }
            }
            DetailField::Reasoning => {
                let Some(index) = self.selected_model_index() else {
                    return PiModelsOutcome::Changed;
                };
                if let Some(model) = self
                    .snapshot
                    .document
                    .providers
                    .get_mut(&provider_name)
                    .and_then(|provider| provider.models.get_mut(index))
                {
                    model.reasoning = cycle_optional_bool(model.reasoning);
                }
            }
            _ => return self.begin_edit_selected(),
        }
        self.notice_success("Draft updated. Press s to save and hot reload.");
        PiModelsOutcome::Changed
    }

    fn arm_delete(&mut self) -> PiModelsOutcome {
        match self.focus {
            PiModelsPane::Providers => {
                if let Some(name) = self.selected_provider_name() {
                    self.confirm = Some(ConfirmAction::DeleteProvider(name.clone()));
                    self.notice_info(format!("Delete provider '{name}' and all its models?"));
                }
            }
            PiModelsPane::Models | PiModelsPane::Details => {
                if let (Some(provider), Some(index), Some(model)) = (
                    self.selected_provider_name(),
                    self.selected_model_index(),
                    self.selected_model(),
                ) {
                    let id = model.id.clone();
                    self.confirm = Some(ConfirmAction::DeleteModel { provider, index });
                    self.notice_info(format!("Delete model '{id}'?"));
                }
            }
        }
        PiModelsOutcome::Changed
    }

    fn save(&mut self) -> PiModelsOutcome {
        if !self.snapshot.is_dirty() {
            self.notice_info("No model configuration changes to save.");
            return PiModelsOutcome::Changed;
        }
        match self.snapshot.save() {
            Ok(report) => {
                let backup = report
                    .backup
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .and_then(|name| name.to_str())
                    .map(|name| format!(" · backup {name}"))
                    .unwrap_or_default();
                self.notice_success(format!(
                    "Saved {}{backup}; reloading Pi model catalog…",
                    report.path.display()
                ));
                PiModelsOutcome::Reload
            }
            Err(error) => {
                self.notice_error(format!("Save failed: {error:#}"));
                PiModelsOutcome::Changed
            }
        }
    }

    fn reload_from_disk(&mut self) -> PiModelsOutcome {
        match self.snapshot.reload_from_disk() {
            Ok(()) => {
                self.clamp_selection();
                self.notice_success("Reloaded models.json from disk.");
            }
            Err(error) => self.notice_error(format!("Refresh failed: {error:#}")),
        }
        PiModelsOutcome::Changed
    }

    fn restore_latest(&mut self) -> PiModelsOutcome {
        match self.snapshot.restore_latest() {
            Ok(report) => {
                self.clamp_selection();
                self.notice_success(format!(
                    "Restored {}; reloading Pi model catalog…",
                    report.restored_from.display()
                ));
                PiModelsOutcome::Reload
            }
            Err(error) => {
                self.notice_error(format!("Restore failed: {error:#}"));
                PiModelsOutcome::Changed
            }
        }
    }

    fn activate_selected(&mut self) -> PiModelsOutcome {
        let (Some(provider), Some(model)) = (self.selected_provider_name(), self.selected_model())
        else {
            self.notice_error("Select a model before activating it.");
            return PiModelsOutcome::Changed;
        };
        if self.snapshot.is_dirty() {
            self.notice_error("Save and hot reload before activating a changed model.");
            return PiModelsOutcome::Changed;
        }
        PiModelsOutcome::Activate(format!("{provider}/{}", model.id))
    }

    pub fn set_current_model(&mut self, model: Option<String>) {
        self.current_model = model;
    }

    pub fn set_error(&mut self, message: impl Into<String>) {
        self.notice_error(message);
    }

    fn field_raw_value(&self, field: DetailField) -> String {
        let provider = self.selected_provider();
        let model = self.selected_model();
        match field {
            DetailField::ProviderName => self.selected_provider_name().unwrap_or_default(),
            DetailField::BaseUrl => provider
                .and_then(|value| value.base_url.clone())
                .unwrap_or_default(),
            DetailField::ProviderApi => provider
                .and_then(|value| value.api.clone())
                .unwrap_or_default(),
            DetailField::ApiKey => provider
                .and_then(|value| value.api_key.clone())
                .unwrap_or_default(),
            DetailField::AuthHeader => {
                format_optional_bool(provider.and_then(|value| value.auth_header))
            }
            DetailField::Headers => serde_json::to_string(
                provider
                    .map(|value| &value.headers)
                    .unwrap_or(&BTreeMap::new()),
            )
            .unwrap_or_else(|_| "{}".to_owned()),
            DetailField::ModelId => model.map(|value| value.id.clone()).unwrap_or_default(),
            DetailField::ModelName => model.map(|value| value.name.clone()).unwrap_or_default(),
            DetailField::ModelApi => model
                .and_then(|value| value.api.clone())
                .unwrap_or_default(),
            DetailField::Reasoning => format_optional_bool(model.and_then(|value| value.reasoning)),
            DetailField::Input => model
                .map(|value| value.input.join(", "))
                .unwrap_or_default(),
            DetailField::ContextWindow => model
                .and_then(|value| value.context_window)
                .map(|value| value.to_string())
                .unwrap_or_default(),
            DetailField::MaxTokens => model
                .and_then(|value| value.max_tokens)
                .map(|value| value.to_string())
                .unwrap_or_default(),
            DetailField::CostInput => model
                .and_then(|value| value.cost.as_ref())
                .map(|cost| cost.input.to_string())
                .unwrap_or_default(),
            DetailField::CostOutput => model
                .and_then(|value| value.cost.as_ref())
                .map(|cost| cost.output.to_string())
                .unwrap_or_default(),
            DetailField::CostCacheRead => model
                .and_then(|value| value.cost.as_ref())
                .map(|cost| cost.cache_read.to_string())
                .unwrap_or_default(),
            DetailField::CostCacheWrite => model
                .and_then(|value| value.cost.as_ref())
                .map(|cost| cost.cache_write.to_string())
                .unwrap_or_default(),
            DetailField::ThinkingLevelMap => serde_json::to_string(
                model
                    .map(|value| &value.thinking_level_map)
                    .unwrap_or(&BTreeMap::new()),
            )
            .unwrap_or_else(|_| "{}".to_owned()),
        }
    }

    fn field_display_value(&self, field: DetailField) -> String {
        if field == DetailField::ApiKey {
            let raw = self.field_raw_value(field);
            return if raw.is_empty() {
                "—".to_owned()
            } else {
                mask_secret(&raw)
            };
        }
        let raw = self.field_raw_value(field);
        if raw.is_empty() {
            "—".to_owned()
        } else {
            raw
        }
    }

    fn commit_field(&mut self, field: DetailField, value: &str) -> Result<()> {
        let provider_name = self
            .selected_provider_name()
            .context("no provider selected")?;
        let model_index = self.selected_model_index();
        match field {
            DetailField::ProviderName => {
                let new_name = value.trim();
                if new_name.is_empty() {
                    bail!("provider name must not be empty");
                }
                if new_name != provider_name
                    && self.snapshot.document.providers.contains_key(new_name)
                {
                    bail!("provider '{new_name}' already exists");
                }
                if new_name != provider_name {
                    let config = self
                        .snapshot
                        .document
                        .providers
                        .remove(&provider_name)
                        .context("selected provider disappeared")?;
                    self.snapshot
                        .document
                        .providers
                        .insert(new_name.to_owned(), config);
                    self.provider_selected = self
                        .provider_names()
                        .iter()
                        .position(|candidate| candidate == new_name)
                        .unwrap_or(0);
                }
            }
            DetailField::BaseUrl => {
                self.provider_mut(&provider_name)?.base_url = optional_text(value)
            }
            DetailField::ProviderApi => {
                self.provider_mut(&provider_name)?.api = optional_text(value)
            }
            DetailField::ApiKey => {
                self.provider_mut(&provider_name)?.api_key = optional_text(value)
            }
            DetailField::AuthHeader | DetailField::Reasoning => {
                bail!("use Space to toggle this field")
            }
            DetailField::Headers => {
                self.provider_mut(&provider_name)?.headers = parse_string_map(value, "headers")?;
            }
            DetailField::ModelId => {
                let index = model_index.context("no model selected")?;
                let id = value.trim();
                if id.is_empty() {
                    bail!("model id must not be empty");
                }
                let provider = self.provider_mut(&provider_name)?;
                if provider
                    .models
                    .iter()
                    .enumerate()
                    .any(|(candidate_index, model)| candidate_index != index && model.id == id)
                {
                    bail!("model id '{id}' already exists in provider '{provider_name}'");
                }
                provider.models[index].id = id.to_owned();
            }
            DetailField::ModelName => {
                let name = value.trim();
                if name.is_empty() {
                    bail!("display name must not be empty");
                }
                self.model_mut(&provider_name, model_index)?.name = name.to_owned();
            }
            DetailField::ModelApi => {
                self.model_mut(&provider_name, model_index)?.api = optional_text(value)
            }
            DetailField::Input => {
                let values = value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                if values.is_empty() {
                    bail!("at least one input modality is required");
                }
                self.model_mut(&provider_name, model_index)?.input = values;
            }
            DetailField::ContextWindow => {
                self.model_mut(&provider_name, model_index)?.context_window =
                    parse_optional_u64(value, "context window")?
            }
            DetailField::MaxTokens => {
                self.model_mut(&provider_name, model_index)?.max_tokens =
                    parse_optional_u64(value, "max tokens")?
            }
            DetailField::CostInput => set_cost(
                self.model_mut(&provider_name, model_index)?,
                CostField::Input,
                parse_cost(value)?,
            ),
            DetailField::CostOutput => set_cost(
                self.model_mut(&provider_name, model_index)?,
                CostField::Output,
                parse_cost(value)?,
            ),
            DetailField::CostCacheRead => set_cost(
                self.model_mut(&provider_name, model_index)?,
                CostField::CacheRead,
                parse_cost(value)?,
            ),
            DetailField::CostCacheWrite => set_cost(
                self.model_mut(&provider_name, model_index)?,
                CostField::CacheWrite,
                parse_cost(value)?,
            ),
            DetailField::ThinkingLevelMap => {
                self.model_mut(&provider_name, model_index)?
                    .thinking_level_map = parse_nullable_string_map(value, "thinking level map")?;
            }
        }
        validate_document(&self.snapshot.document)?;
        Ok(())
    }

    fn provider_mut(&mut self, name: &str) -> Result<&mut PiProviderConfig> {
        self.snapshot
            .document
            .providers
            .get_mut(name)
            .context("selected provider disappeared")
    }

    fn model_mut(&mut self, provider: &str, index: Option<usize>) -> Result<&mut PiModelConfig> {
        let index = index.context("no model selected")?;
        self.provider_mut(provider)?
            .models
            .get_mut(index)
            .context("selected model disappeared")
    }

    fn focus_for_point(&mut self, x: u16, y: u16) {
        if self.provider_rect.is_some_and(|rect| contains(rect, x, y)) {
            self.focus = PiModelsPane::Providers;
        } else if self.model_rect.is_some_and(|rect| contains(rect, x, y)) {
            self.focus = PiModelsPane::Models;
        } else if self.detail_rect.is_some_and(|rect| contains(rect, x, y)) {
            self.focus = PiModelsPane::Details;
        }
    }

    fn notice_info(&mut self, message: impl Into<String>) {
        self.notice = Some(Notice {
            kind: NoticeKind::Info,
            message: message.into(),
        });
    }

    fn notice_success(&mut self, message: impl Into<String>) {
        self.notice = Some(Notice {
            kind: NoticeKind::Success,
            message: message.into(),
        });
    }

    fn notice_error(&mut self, message: impl Into<String>) {
        self.notice = Some(Notice {
            kind: NoticeKind::Error,
            message: message.into(),
        });
    }
}

pub fn render_pi_models_modal(
    buf: &mut Buffer,
    area: Rect,
    state: &mut PiModelsModalState,
    compact: bool,
) {
    let config = ModalWindowConfig {
        title: "Pi models",
        tabs: None,
        shortcuts: &SHORTCUTS,
        sizing: ModalSizing {
            width_pct: 0.96,
            max_width: 168,
            min_width: 58,
            v_margin: if compact { 0 } else { 3 },
            h_pad: 1,
            v_pad: if compact { 0 } else { 1 },
            footer_lines: 2,
        },
        fold_info: None,
    };
    let theme = Theme::current();
    let Some(content) = render_modal_window(buf, area, &mut state.window, &config, &theme) else {
        return;
    };
    if content.content.width == 0 || content.content.height < 5 {
        return;
    }

    state.provider_rows.clear();
    state.model_rows.clear();
    state.detail_rows.clear();

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(content.content);
    let body = vertical[0];
    let status = vertical[1];

    if body.width >= 82 {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(24),
                Constraint::Percentage(31),
                Constraint::Percentage(45),
            ])
            .split(body);
        state.provider_rect = Some(columns[0]);
        state.model_rect = Some(columns[1]);
        state.detail_rect = Some(columns[2]);
        render_provider_pane(buf, columns[0], state, &theme);
        render_model_pane(buf, columns[1], state, &theme);
        render_detail_pane(buf, columns[2], state, &theme);
    } else {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
            .split(body);
        let lists = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(43), Constraint::Percentage(57)])
            .split(rows[0]);
        state.provider_rect = Some(lists[0]);
        state.model_rect = Some(lists[1]);
        state.detail_rect = Some(rows[1]);
        render_provider_pane(buf, lists[0], state, &theme);
        render_model_pane(buf, lists[1], state, &theme);
        render_detail_pane(buf, rows[1], state, &theme);
    }
    render_status(buf, status, state, &theme);
    if state.confirm.is_some() {
        render_confirmation(buf, content.content, state, &theme);
    }
}

fn render_provider_pane(
    buf: &mut Buffer,
    area: Rect,
    state: &mut PiModelsModalState,
    theme: &Theme,
) {
    let title = format!(" Providers · {} ", state.snapshot.document.providers.len());
    let inner = render_pane_block(
        buf,
        area,
        &title,
        state.focus == PiModelsPane::Providers,
        theme,
    );
    if inner.height == 0 {
        return;
    }
    let query = if state.search_pane == PiModelsPane::Providers {
        &state.search_query
    } else {
        ""
    };
    let list = render_search_line(
        buf,
        inner,
        query,
        state.search_active && state.search_pane == PiModelsPane::Providers,
        theme,
    );
    let names = state.provider_names();
    state.provider_selected = state.provider_selected.min(names.len().saturating_sub(1));
    state.provider_scroll = ensure_visible(
        state.provider_scroll,
        state.provider_selected,
        list.height as usize,
        names.len(),
    );
    if names.is_empty() {
        write_line(
            buf,
            list.x,
            list.y,
            list.width,
            "No providers · press n",
            Style::default().fg(theme.gray_dim),
        );
        return;
    }
    for (offset, name) in names
        .iter()
        .skip(state.provider_scroll)
        .take(list.height as usize)
        .enumerate()
    {
        let index = state.provider_scroll + offset;
        let rect = Rect::new(list.x, list.y + offset as u16, list.width, 1);
        state.provider_rows.push((rect, index));
        let count = state
            .snapshot
            .document
            .providers
            .get(name)
            .map(|provider| provider.models.len())
            .unwrap_or(0);
        let style = row_style(
            index == state.provider_selected,
            state.focus == PiModelsPane::Providers,
            theme,
        );
        write_line(
            buf,
            rect.x,
            rect.y,
            rect.width,
            &format!(" {name}  · {count}"),
            style,
        );
    }
}

fn render_model_pane(buf: &mut Buffer, area: Rect, state: &mut PiModelsModalState, theme: &Theme) {
    let provider = state
        .selected_provider_name()
        .unwrap_or_else(|| "No provider".to_owned());
    let title = format!(" Models · {provider} ");
    let inner = render_pane_block(
        buf,
        area,
        &title,
        state.focus == PiModelsPane::Models,
        theme,
    );
    if inner.height == 0 {
        return;
    }
    let query = if state.search_pane == PiModelsPane::Models {
        &state.search_query
    } else {
        ""
    };
    let list = render_search_line(
        buf,
        inner,
        query,
        state.search_active && state.search_pane == PiModelsPane::Models,
        theme,
    );
    let indexes = state.model_indexes();
    state.model_selected = state.model_selected.min(indexes.len().saturating_sub(1));
    state.model_scroll = ensure_visible(
        state.model_scroll,
        state.model_selected,
        list.height as usize,
        indexes.len(),
    );
    if indexes.is_empty() {
        write_line(
            buf,
            list.x,
            list.y,
            list.width,
            "No models · press n",
            Style::default().fg(theme.gray_dim),
        );
        return;
    }
    let provider_name = state.selected_provider_name().unwrap_or_default();
    let models = indexes
        .iter()
        .filter_map(|index| state.selected_provider()?.models.get(*index).cloned())
        .collect::<Vec<_>>();
    for (offset, model) in models
        .iter()
        .skip(state.model_scroll)
        .take(list.height as usize)
        .enumerate()
    {
        let filtered_index = state.model_scroll + offset;
        let rect = Rect::new(list.x, list.y + offset as u16, list.width, 1);
        state.model_rows.push((rect, filtered_index));
        let active = state
            .current_model
            .as_ref()
            .is_some_and(|current| model_matches_current(current, &provider_name, &model.id));
        let marker = if active {
            "●"
        } else if model.reasoning == Some(true) {
            "◆"
        } else {
            "·"
        };
        let style = row_style(
            filtered_index == state.model_selected,
            state.focus == PiModelsPane::Models,
            theme,
        );
        write_line(
            buf,
            rect.x,
            rect.y,
            rect.width,
            &format!(" {marker} {}  {}", model.id, model.name),
            style,
        );
    }
}

fn render_detail_pane(buf: &mut Buffer, area: Rect, state: &mut PiModelsModalState, theme: &Theme) {
    let selected = state
        .selected_model()
        .map(|model| model.id.as_str())
        .unwrap_or("Provider");
    let title = format!(" Details · {selected} ");
    let inner = render_pane_block(
        buf,
        area,
        &title,
        state.focus == PiModelsPane::Details,
        theme,
    );
    if inner.height == 0 {
        return;
    }
    let query = if state.search_pane == PiModelsPane::Details {
        &state.search_query
    } else {
        ""
    };
    let list = render_search_line(
        buf,
        inner,
        query,
        state.search_active && state.search_pane == PiModelsPane::Details,
        theme,
    );
    let fields = state.detail_fields();
    state.detail_selected = state.detail_selected.min(fields.len().saturating_sub(1));
    state.detail_scroll = ensure_visible(
        state.detail_scroll,
        state.detail_selected,
        list.height as usize,
        fields.len(),
    );
    if fields.is_empty() {
        write_line(
            buf,
            list.x,
            list.y,
            list.width,
            "Select or create a provider.",
            Style::default().fg(theme.gray_dim),
        );
        return;
    }
    let label_width = (list.width as usize / 3).clamp(14, 24);
    for (offset, field) in fields
        .iter()
        .skip(state.detail_scroll)
        .take(list.height as usize)
        .enumerate()
    {
        let index = state.detail_scroll + offset;
        let rect = Rect::new(list.x, list.y + offset as u16, list.width, 1);
        state.detail_rows.push((rect, index));
        let selected = index == state.detail_selected;
        let mut value = state.field_display_value(*field);
        if let Some(editor) = state
            .editor
            .as_ref()
            .filter(|editor| editor.field == *field)
        {
            value = editor_display(editor);
        }
        let text = format!(" {:<label_width$}  {value}", field.label());
        let style = row_style(selected, state.focus == PiModelsPane::Details, theme);
        write_line(buf, rect.x, rect.y, rect.width, &text, style);
    }
}

fn render_pane_block(
    buf: &mut Buffer,
    area: Rect,
    title: &str,
    focused: bool,
    theme: &Theme,
) -> Rect {
    if area.width < 3 || area.height < 3 {
        return area;
    }
    let border = if focused {
        theme.fuzzy_accent
    } else {
        theme.gray_dim
    };
    let title_style = if focused {
        Style::default()
            .fg(theme.fuzzy_accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border))
        .title(Line::styled(title.to_owned(), title_style));
    let inner = block.inner(area);
    block.render(area, buf);
    inner
}

fn render_search_line(
    buf: &mut Buffer,
    area: Rect,
    query: &str,
    active: bool,
    theme: &Theme,
) -> Rect {
    if area.height == 0 {
        return area;
    }
    let text = if query.is_empty() {
        if active {
            "/ ▌".to_owned()
        } else {
            "/ search".to_owned()
        }
    } else {
        format!("/ {query}{}", if active { "▌" } else { "" })
    };
    write_line(
        buf,
        area.x,
        area.y,
        area.width,
        &text,
        Style::default().fg(if active {
            theme.fuzzy_accent
        } else {
            theme.gray_dim
        }),
    );
    Rect::new(
        area.x,
        area.y.saturating_add(1),
        area.width,
        area.height.saturating_sub(1),
    )
}

fn render_status(buf: &mut Buffer, area: Rect, state: &PiModelsModalState, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let dirty = if state.snapshot.is_dirty() {
        "modified"
    } else {
        "saved"
    };
    let provider_count = state.snapshot.document.providers.len();
    let model_count = state
        .snapshot
        .document
        .providers
        .values()
        .map(|provider| provider.models.len())
        .sum::<usize>();
    let current = state.current_model.as_deref().unwrap_or("not reported");
    write_line(
        buf,
        area.x,
        area.y,
        area.width,
        &format!(
            "{} · {dirty} · {provider_count} providers / {model_count} models",
            state.snapshot.path.display()
        ),
        Style::default().fg(if state.snapshot.is_dirty() {
            theme.fuzzy_accent
        } else {
            theme.gray_dim
        }),
    );
    if area.height > 1 {
        write_line(
            buf,
            area.x,
            area.y + 1,
            area.width,
            &format!("Active model: {current}"),
            Style::default().fg(theme.gray_dim),
        );
    }
    if area.height > 2 {
        if let Some(notice) = &state.notice {
            let color = match notice.kind {
                NoticeKind::Info => theme.gray_bright,
                NoticeKind::Success => theme.accent_success,
                NoticeKind::Error => theme.accent_error,
            };
            write_line(
                buf,
                area.x,
                area.y + 2,
                area.width,
                &notice.message,
                Style::default().fg(color),
            );
        } else {
            write_line(
                buf,
                area.x,
                area.y + 2,
                area.width,
                "Space cycles optional booleans · u restores latest backup · r refreshes disk",
                Style::default().fg(theme.gray_dim),
            );
        }
    }
}

fn render_confirmation(buf: &mut Buffer, area: Rect, state: &PiModelsModalState, theme: &Theme) {
    let width = area.width.min(74).max(32);
    let height = 7.min(area.height).max(3);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let rect = Rect::new(x, y, width, height);
    Clear.render(rect, buf);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.fuzzy_accent))
        .title(Line::styled(
            " Confirm ",
            Style::default()
                .fg(theme.fuzzy_accent)
                .add_modifier(Modifier::BOLD),
        ));
    let message = state
        .notice
        .as_ref()
        .map(|notice| notice.message.as_str())
        .unwrap_or("Continue?");
    Paragraph::new(Text::from(vec![
        Line::from(message.to_owned()),
        Line::from(""),
        Line::styled(
            "y confirm    n/Esc cancel",
            Style::default().fg(theme.gray_bright),
        ),
    ]))
    .style(Style::default().fg(theme.text_primary))
    .block(block)
    .wrap(Wrap { trim: true })
    .render(rect, buf);
}

fn row_style(selected: bool, focused: bool, theme: &Theme) -> Style {
    if selected {
        Style::default()
            .fg(if focused {
                theme.fuzzy_accent
            } else {
                theme.text_primary
            })
            .bg(theme.bg_visual)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.gray_bright)
    }
}

fn write_line(buf: &mut Buffer, x: u16, y: u16, width: u16, text: &str, style: Style) {
    if width > 0 {
        buf.set_line(x, y, &Line::styled(text, style), width);
    }
}

fn char_to_byte(value: &str, char_index: usize) -> usize {
    value
        .char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(value.len())
}

fn editor_display(editor: &FieldEditor) -> String {
    let mut value = if editor.field == DetailField::ApiKey {
        "•".repeat(editor.buffer.chars().count())
    } else {
        editor.buffer.clone()
    };
    let byte = char_to_byte(&value, editor.cursor.min(value.chars().count()));
    value.insert_str(byte, "▌");
    value
}

fn contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x
        && x < rect.x.saturating_add(rect.width)
        && y >= rect.y
        && y < rect.y.saturating_add(rect.height)
}

fn ensure_visible(scroll: usize, selected: usize, viewport: usize, len: usize) -> usize {
    if len == 0 || viewport == 0 {
        return 0;
    }
    let mut scroll = scroll.min(len.saturating_sub(1));
    if selected < scroll {
        scroll = selected;
    }
    if selected >= scroll + viewport {
        scroll = selected + 1 - viewport;
    }
    scroll.min(len.saturating_sub(viewport.min(len)))
}

fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn parse_optional_u64(value: &str, label: &str) -> Result<Option<u64>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let number = value
        .parse::<u64>()
        .with_context(|| format!("{label} must be an integer"))?;
    if number == 0 {
        bail!("{label} must be greater than zero");
    }
    Ok(Some(number))
}

fn parse_cost(value: &str) -> Result<f64> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(0.0);
    }
    let number = value.parse::<f64>().context("cost must be a number")?;
    if !number.is_finite() || number < 0.0 {
        bail!("cost must be finite and non-negative");
    }
    Ok(number)
}

fn parse_string_map(value: &str, label: &str) -> Result<BTreeMap<String, String>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(BTreeMap::new());
    }
    serde_json::from_str(value)
        .with_context(|| format!("{label} must be a JSON object with string values"))
}

fn parse_nullable_string_map(value: &str, label: &str) -> Result<BTreeMap<String, Option<String>>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(BTreeMap::new());
    }
    serde_json::from_str(value)
        .with_context(|| format!("{label} must be a JSON object with string or null values"))
}

fn cycle_optional_bool(value: Option<bool>) -> Option<bool> {
    match value {
        None => Some(true),
        Some(true) => Some(false),
        Some(false) => None,
    }
}

fn format_optional_bool(value: Option<bool>) -> String {
    match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "inherit",
    }
    .to_owned()
}

fn mask_secret(value: &str) -> String {
    let suffix = value
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("••••{suffix}")
}

fn unique_provider_name(providers: &BTreeMap<String, PiProviderConfig>, base: &str) -> String {
    if !providers.contains_key(base) {
        return base.to_owned();
    }
    (2..)
        .map(|index| format!("{base}-{index}"))
        .find(|name| !providers.contains_key(name))
        .expect("unbounded names")
}

fn unique_model_id(provider: &PiProviderConfig, base: &str) -> String {
    if !provider.models.iter().any(|model| model.id == base) {
        return base.to_owned();
    }
    (2..)
        .map(|index| format!("{base}-{index}"))
        .find(|id| !provider.models.iter().any(|model| model.id == *id))
        .expect("unbounded ids")
}

fn model_matches_current(current: &str, provider: &str, model: &str) -> bool {
    current.eq_ignore_ascii_case(model)
        || current.eq_ignore_ascii_case(&format!("{provider}/{model}"))
        || current.eq_ignore_ascii_case(&format!("{provider}::{model}"))
}

enum CostField {
    Input,
    Output,
    CacheRead,
    CacheWrite,
}

fn set_cost(model: &mut PiModelConfig, field: CostField, value: f64) {
    let cost = model.cost.get_or_insert_with(PiModelCost::default);
    match field {
        CostField::Input => cost.input = value,
        CostField::Output => cost.output = value,
        CostField::CacheRead => cost.cache_read = value,
        CostField::CacheWrite => cost.cache_write = value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn state() -> PiModelsModalState {
        let mut document = crate::pi_model_config::PiModelsFile::default();
        document.providers.insert(
            "demo".to_owned(),
            PiProviderConfig {
                models: vec![PiModelConfig {
                    id: "alpha".to_owned(),
                    name: "Alpha".to_owned(),
                    input: vec!["text".to_owned()],
                    ..PiModelConfig::default()
                }],
                ..PiProviderConfig::default()
            },
        );
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.keep().join("models.json");
        std::fs::write(
            &path,
            format!("{}\n", serde_json::to_string_pretty(&document).unwrap()),
        )
        .unwrap();
        let snapshot = PiModelConfigSnapshot::load_from_path(PathBuf::from(path)).unwrap();
        PiModelsModalState::from_snapshot(snapshot, Some("demo::alpha".to_owned()))
    }

    #[test]
    fn new_model_marks_draft_dirty() {
        let mut state = state();
        state.focus = PiModelsPane::Models;
        assert_eq!(
            state.handle_key(&KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)),
            PiModelsOutcome::Changed
        );
        assert_eq!(state.selected_provider().unwrap().models.len(), 2);
        assert!(state.snapshot.is_dirty());
    }

    #[test]
    fn activation_requires_clean_document_and_uses_provider_path() {
        let mut state = state();
        state.focus = PiModelsPane::Models;
        assert_eq!(
            state.handle_key(&KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
            PiModelsOutcome::Activate("demo/alpha".to_owned())
        );
        state
            .snapshot
            .document
            .providers
            .get_mut("demo")
            .unwrap()
            .models[0]
            .name = "Changed".to_owned();
        assert_eq!(
            state.handle_key(&KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
            PiModelsOutcome::Changed
        );
    }

    #[test]
    fn optional_boolean_cycles_inherit_true_false() {
        assert_eq!(cycle_optional_bool(None), Some(true));
        assert_eq!(cycle_optional_bool(Some(true)), Some(false));
        assert_eq!(cycle_optional_bool(Some(false)), None);
    }
}
