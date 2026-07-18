//! Native Pager modal for Pi resource configuration.
//!
//! Resource discovery and settings mutation live in [`crate::pi_resource_config`].
//! This module owns only native Pager presentation and interaction.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;

use crate::pi_resource_config::{
    PiProjectOverride, PiResource, PiResourceCatalog, PiResourceOrigin, PiResourceScope,
};
use crate::scrollback::blocks::markdown_content::MarkdownContent;
use crate::theme::Theme;
use crate::views::modal_window::{
    ModalSizing, ModalWindowConfig, ModalWindowState, Shortcut, render_modal_window,
};

const TABS: [&str; 2] = ["Global", "Project"];
const SHORTCUTS: [Shortcut<'static>; 6] = [
    Shortcut { label: "↑/↓ navigate", clickable: false, id: 0 },
    Shortcut { label: "←/→ fold", clickable: false, id: 0 },
    Shortcut { label: "Space toggle", clickable: false, id: 0 },
    Shortcut { label: "/ search", clickable: false, id: 0 },
    Shortcut { label: "Tab scope", clickable: false, id: 0 },
    Shortcut { label: "Esc close", clickable: false, id: 0 },
];

#[derive(Clone)]
enum PiConfigRow {
    Source {
        id: String,
        label: String,
        resource_count: usize,
        preview: PiResource,
    },
    ResourceType { label: String, preview: PiResource },
    Resource(PiResource),
}

impl PiConfigRow {
    fn is_source(&self) -> bool {
        matches!(self, Self::Source { .. })
    }

    fn preview_resource(&self) -> &PiResource {
        match self {
            Self::Source { preview, .. } | Self::ResourceType { preview, .. } | Self::Resource(preview) => preview,
        }
    }
}

#[derive(Clone, Default)]
struct PackagePreview {
    key: String,
    title: String,
    manifest: Vec<String>,
    readme: String,
}

pub struct PiConfigModalState {
    pub window: ModalWindowState,
    catalog: PiResourceCatalog,
    scope: PiResourceScope,
    selected: usize,
    scroll: usize,
    folded_sources: HashSet<String>,
    search_query: String,
    search_active: bool,
    list_rect: Option<Rect>,
    search_rect: Option<Rect>,
    list_viewport: usize,
    preview_rect: Option<Rect>,
    preview_scroll: usize,
    preview: PackagePreview,
    notice: Option<String>,
}

pub enum PiConfigOutcome {
    Close,
    Changed,
}

impl PiConfigModalState {
    pub fn open(cwd: PathBuf) -> Result<Self> {
        let catalog = PiResourceCatalog::load(cwd)?;
        let mut state = Self {
            window: ModalWindowState::new(),
            catalog,
            scope: PiResourceScope::User,
            selected: 0,
            scroll: 0,
            folded_sources: HashSet::new(),
            search_query: String::new(),
            search_active: false,
            list_rect: None,
            search_rect: None,
            list_viewport: 1,
            preview_rect: None,
            preview_scroll: 0,
            preview: PackagePreview::default(),
            notice: None,
        };
        state.fold_all_sources();
        state.refresh_preview();
        Ok(state)
    }

    pub fn select_tab(&mut self, index: usize) {
        if index == 1 && self.catalog.project_trusted {
            self.scope = PiResourceScope::Project;
        } else {
            self.scope = PiResourceScope::User;
        }
        self.selected = 0;
        self.scroll = 0;
        self.fold_all_sources();
        self.refresh_preview();
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> PiConfigOutcome {
        if key.kind == KeyEventKind::Release {
            return PiConfigOutcome::Changed;
        }
        if key.kind == KeyEventKind::Repeat
            && matches!(key.code, KeyCode::Char(' ') | KeyCode::Enter)
        {
            return PiConfigOutcome::Changed;
        }
        if self.search_active {
            return self.handle_search_key(key);
        }

        match key.code {
            KeyCode::Esc | KeyCode::F(2) => PiConfigOutcome::Close,
            KeyCode::Tab if self.catalog.project_trusted => {
                self.select_tab(match self.scope {
                    PiResourceScope::User => 1,
                    PiResourceScope::Project => 0,
                });
                PiConfigOutcome::Changed
            }
            KeyCode::Char('/') if key.modifiers.is_empty() => {
                self.search_active = true;
                PiConfigOutcome::Changed
            }
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                self.move_selection(-1);
                PiConfigOutcome::Changed
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                self.move_selection(1);
                PiConfigOutcome::Changed
            }
            KeyCode::PageUp => {
                self.page(-1);
                PiConfigOutcome::Changed
            }
            KeyCode::PageDown => {
                self.page(1);
                PiConfigOutcome::Changed
            }
            KeyCode::Home | KeyCode::Char('g') if key.modifiers.is_empty() => {
                self.selected = 0;
                self.scroll = 0;
                self.refresh_preview();
                PiConfigOutcome::Changed
            }
            KeyCode::End | KeyCode::Char('G') if key.modifiers.is_empty() => {
                self.selected = self.visible_rows().len().saturating_sub(1);
                self.ensure_visible();
                self.refresh_preview();
                PiConfigOutcome::Changed
            }
            KeyCode::Left | KeyCode::Char('h') if key.modifiers.is_empty() => {
                self.set_selected_source_folded(true);
                PiConfigOutcome::Changed
            }
            KeyCode::Right | KeyCode::Char('l') if key.modifiers.is_empty() => {
                self.set_selected_source_folded(false);
                PiConfigOutcome::Changed
            }
            KeyCode::Char(' ') | KeyCode::Enter if key.modifiers.is_empty() => {
                self.activate_selected();
                PiConfigOutcome::Changed
            }
            KeyCode::Char('r') if key.modifiers.is_empty() => {
                self.refresh();
                PiConfigOutcome::Changed
            }
            KeyCode::Char(character) if key.modifiers.is_empty() => {
                self.search_active = true;
                self.search_query.push(character);
                self.reset_after_filter_change();
                PiConfigOutcome::Changed
            }
            _ => PiConfigOutcome::Changed,
        }
    }

    pub fn handle_mouse(&mut self, mouse: &MouseEvent) -> PiConfigOutcome {
        let in_preview = self
            .preview_rect
            .is_some_and(|rect| rect.contains((mouse.column, mouse.row).into()));
        match mouse.kind {
            MouseEventKind::ScrollUp if in_preview => {
                self.preview_scroll = self.preview_scroll.saturating_sub(3);
                PiConfigOutcome::Changed
            }
            MouseEventKind::ScrollDown if in_preview => {
                self.preview_scroll = self.preview_scroll.saturating_add(3);
                PiConfigOutcome::Changed
            }
            MouseEventKind::ScrollUp => {
                self.move_selection(-3);
                PiConfigOutcome::Changed
            }
            MouseEventKind::ScrollDown => {
                self.move_selection(3);
                PiConfigOutcome::Changed
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if self
                    .search_rect
                    .is_some_and(|rect| rect.contains((mouse.column, mouse.row).into()))
                {
                    self.search_active = true;
                    return PiConfigOutcome::Changed;
                }
                let Some(index) = self.hit_test_row(mouse.column, mouse.row) else {
                    return PiConfigOutcome::Changed;
                };
                let was_selected = self.selected == index;
                self.selected = index;
                self.ensure_visible();
                if self.visible_rows().get(index).is_some_and(PiConfigRow::is_source) {
                    self.toggle_selected_source();
                } else if was_selected {
                    self.toggle_selected_resource();
                } else {
                    self.refresh_preview();
                }
                PiConfigOutcome::Changed
            }
            _ => PiConfigOutcome::Changed,
        }
    }

    fn handle_search_key(&mut self, key: &KeyEvent) -> PiConfigOutcome {
        match key.code {
            KeyCode::Esc => {
                if self.search_query.is_empty() {
                    self.search_active = false;
                } else {
                    self.search_query.clear();
                    self.reset_after_filter_change();
                }
            }
            KeyCode::Enter => self.search_active = false,
            KeyCode::Backspace => {
                if self.search_query.pop().is_some() {
                    self.reset_after_filter_change();
                }
            }
            KeyCode::Char(character) if key.modifiers.is_empty() => {
                self.search_query.push(character);
                self.reset_after_filter_change();
            }
            _ => {}
        }
        PiConfigOutcome::Changed
    }

    fn visible_rows(&self) -> Vec<PiConfigRow> {
        let mut groups: BTreeMap<String, (String, Vec<PiResource>)> = BTreeMap::new();
        for resource in self.catalog.resources_for_scope(self.scope) {
            if !self.matches_search(resource) {
                continue;
            }
            let id = source_id(resource);
            groups
                .entry(id)
                .or_insert_with(|| (source_label(resource), Vec::new()))
                .1
                .push(resource.clone());
        }

        let searching = !self.search_query.trim().is_empty();
        let mut rows = Vec::new();
        for (id, (label, mut resources)) in groups {
            resources.sort_by_cached_key(|resource| {
                (resource.resource_type, resource.display_name().to_lowercase())
            });
            let preview = resources[0].clone();
            let resource_count = resources.len();
            let folded = !searching && self.folded_sources.contains(&id);
            rows.push(PiConfigRow::Source {
                id,
                label,
                resource_count,
                preview,
            });
            if !folded {
                for resource_type in [
                    crate::pi_resource_config::PiResourceType::Extensions,
                    crate::pi_resource_config::PiResourceType::Skills,
                    crate::pi_resource_config::PiResourceType::Prompts,
                    crate::pi_resource_config::PiResourceType::Themes,
                ] {
                    let typed = resources
                        .iter()
                        .filter(|resource| resource.resource_type == resource_type)
                        .cloned()
                        .collect::<Vec<_>>();
                    if let Some(preview) = typed.first().cloned() {
                        rows.push(PiConfigRow::ResourceType {
                            label: resource_type.label().to_owned(),
                            preview,
                        });
                        rows.extend(typed.into_iter().map(PiConfigRow::Resource));
                    }
                }
            }
        }
        rows
    }

    fn source_ids(&self) -> HashSet<String> {
        self.catalog
            .resources_for_scope(self.scope)
            .into_iter()
            .map(source_id)
            .collect()
    }

    fn fold_all_sources(&mut self) {
        self.folded_sources.extend(self.source_ids());
        self.clamp_selected();
    }

    fn matches_search(&self, resource: &PiResource) -> bool {
        let query = self.search_query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        [
            resource.display_name(),
            resource.resource_type.label().to_owned(),
            source_label(resource),
            resource.path.display().to_string(),
        ]
        .into_iter()
        .any(|value| value.to_lowercase().contains(&query))
    }

    fn activate_selected(&mut self) {
        if self
            .visible_rows()
            .get(self.selected)
            .is_some_and(PiConfigRow::is_source)
        {
            self.toggle_selected_source();
        } else {
            self.toggle_selected_resource();
        }
    }

    fn toggle_selected_source(&mut self) {
        let Some(PiConfigRow::Source { id, .. }) = self.visible_rows().get(self.selected).cloned()
        else {
            return;
        };
        if !self.folded_sources.insert(id.clone()) {
            self.folded_sources.remove(&id);
        }
        self.clamp_selected();
        self.ensure_visible();
        self.refresh_preview();
    }

    fn set_selected_source_folded(&mut self, folded: bool) {
        let Some(PiConfigRow::Source { id, .. }) = self.visible_rows().get(self.selected).cloned()
        else {
            return;
        };
        if folded {
            self.folded_sources.insert(id);
        } else {
            self.folded_sources.remove(&id);
        }
        self.clamp_selected();
        self.refresh_preview();
    }

    fn toggle_selected_resource(&mut self) {
        let Some(PiConfigRow::Resource(resource)) = self.visible_rows().get(self.selected).cloned()
        else {
            return;
        };
        let result = match self.scope {
            PiResourceScope::User => self.catalog.set_global_enabled(&resource, !resource.enabled),
            PiResourceScope::Project => self.catalog.set_project_override(
                &resource,
                next_override(resource.project_override, resource.inherited_enabled),
            ),
        };
        match result {
            Ok(()) => {
                self.notice =
                    Some("Saved to Pi settings · restart grok-pi or use Pi /reload".to_owned());
                self.refresh();
            }
            Err(error) => self.notice = Some(format!("Pi config: {error:#}")),
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let len = self.visible_rows().len() as isize;
        if len == 0 {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
        self.ensure_visible();
        self.refresh_preview();
    }

    fn page(&mut self, delta: isize) {
        let len = self.visible_rows().len();
        if len == 0 {
            return;
        }
        let page = self.list_viewport.max(1);
        if delta < 0 {
            self.selected = self.selected.saturating_sub(page);
        } else {
            self.selected = (self.selected + page).min(len - 1);
        }
        self.ensure_visible();
        self.refresh_preview();
    }

    fn reset_after_filter_change(&mut self) {
        self.selected = 0;
        self.scroll = 0;
        self.clamp_selected();
        self.refresh_preview();
    }

    fn clamp_selected(&mut self) {
        self.selected = self.selected.min(self.visible_rows().len().saturating_sub(1));
    }

    fn ensure_visible(&mut self) {
        let viewport = self.list_viewport.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll.saturating_add(viewport) {
            self.scroll = self.selected + 1 - viewport;
        }
    }

    fn hit_test_row(&self, column: u16, row: u16) -> Option<usize> {
        let list = self.list_rect?;
        if !list.contains((column, row).into()) {
            return None;
        }
        let index = self.scroll + (row - list.y) as usize;
        (index < self.visible_rows().len()).then_some(index)
    }

    fn refresh(&mut self) {
        match PiResourceCatalog::load(self.catalog.cwd.clone()) {
            Ok(catalog) => {
                self.catalog = catalog;
                if self.scope == PiResourceScope::Project && !self.catalog.project_trusted {
                    self.scope = PiResourceScope::User;
                }
                self.fold_all_sources();
                self.refresh_preview();
            }
            Err(error) => self.notice = Some(format!("Pi config: {error:#}")),
        }
    }

    fn refresh_preview(&mut self) {
        let Some(resource) = self
            .visible_rows()
            .get(self.selected)
            .map(PiConfigRow::preview_resource)
            .cloned()
        else {
            self.preview = PackagePreview::default();
            return;
        };
        let key = format!("{}:{}", resource.base_dir.display(), resource.path.display());
        if self.preview.key == key {
            return;
        }
        self.preview_scroll = 0;
        self.preview = package_preview(resource, key);
    }
}

fn next_override(current: PiProjectOverride, inherited_enabled: bool) -> PiProjectOverride {
    match (current, inherited_enabled) {
        (PiProjectOverride::Inherit, true) => PiProjectOverride::Unload,
        (PiProjectOverride::Inherit, false) => PiProjectOverride::Load,
        (PiProjectOverride::Unload, true) => PiProjectOverride::Load,
        (PiProjectOverride::Unload, false) => PiProjectOverride::Inherit,
        (PiProjectOverride::Load, true) => PiProjectOverride::Inherit,
        (PiProjectOverride::Load, false) => PiProjectOverride::Unload,
    }
}

fn source_id(resource: &PiResource) -> String {
    format!(
        "{}:{}:{}",
        resource.origin.label(),
        resource.source,
        resource.base_dir.display()
    )
}

fn source_label(resource: &PiResource) -> String {
    match resource.origin {
        PiResourceOrigin::Package if resource.source.starts_with("https://github.com/") => {
            let repo = resource
                .source
                .trim_start_matches("https://github.com/")
                .trim_end_matches(".git");
            format!("GitHub · {repo}")
        }
        PiResourceOrigin::Package if resource.source.starts_with("npm:") => {
            format!("npm · {}", resource.source.trim_start_matches("npm:"))
        }
        PiResourceOrigin::Package => format!("Package · {}", resource.source),
        PiResourceOrigin::Auto => format!("Auto-discovered · {}", resource.base_dir.display()),
        PiResourceOrigin::Settings => format!("Settings path · {}", resource.base_dir.display()),
    }
}

fn package_preview(resource: PiResource, key: String) -> PackagePreview {
    let root = preview_root(&resource);
    let manifest = fs::read_to_string(root.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .map_or_else(
            || vec!["No package.json at this resource root.".to_owned()],
            |json| useful_manifest_lines(&json),
        );
    let readme = ["README.md", "Readme.md", "readme.md"]
        .into_iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file())
        .map_or_else(
            || "_No README found._".to_owned(),
            |path| preview_file_markdown(&path),
        );
    PackagePreview {
        key,
        title: format!("{} · {}", source_label(&resource), resource.display_name()),
        manifest,
        readme,
    }
}

fn preview_root(resource: &PiResource) -> PathBuf {
    if resource.origin == PiResourceOrigin::Package {
        return resource.base_dir.clone();
    }
    let mut current = resource.path.parent().unwrap_or(&resource.base_dir);
    while current.starts_with(&resource.base_dir) {
        if current.join("package.json").is_file() {
            return current.to_path_buf();
        }
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent;
    }
    resource.base_dir.clone()
}

fn useful_manifest_lines(json: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();
    for key in ["name", "version", "description", "homepage", "repository"] {
        if let Some(value) = json.get(key) {
            let rendered = match value {
                serde_json::Value::String(value) => value.clone(),
                serde_json::Value::Object(object) if key == "repository" => object
                    .get("url")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("repository object")
                    .to_owned(),
                _ => value.to_string(),
            };
            lines.push(format!("{key}: {rendered}"));
        }
    }
    if let Some(pi) = json.get("pi") {
        lines.push(format!("pi: {pi}"));
    }
    if lines.is_empty() {
        lines.push("package.json has no display metadata.".to_owned());
    }
    lines
}

fn preview_file_markdown(path: &Path) -> String {
    let Ok(file) = fs::File::open(path) else {
        return "_README could not be read._".to_owned();
    };
    let mut text = String::new();
    if file.take(12_000).read_to_string(&mut text).is_err() {
        return "_README could not be read._".to_owned();
    }
    if text.trim().is_empty() {
        "_README is empty._".to_owned()
    } else {
        text
    }
}

pub fn render_pi_config_modal(
    buf: &mut Buffer,
    area: Rect,
    state: &mut PiConfigModalState,
    compact: bool,
) {
    state.window.active_tab = match state.scope {
        PiResourceScope::User => 0,
        PiResourceScope::Project => 1,
    };
    let config = ModalWindowConfig {
        title: "Pi resources",
        tabs: state.catalog.project_trusted.then_some(&TABS),
        shortcuts: &SHORTCUTS,
        sizing: ModalSizing::large().with_compact(compact),
        fold_info: None,
    };
    let theme = Theme::current();
    let Some(content) = render_modal_window(buf, area, &mut state.window, &config, &theme) else {
        state.list_rect = None;
        state.search_rect = None;
        return;
    };
    if content.content.height == 0 || content.content.width == 0 {
        state.list_rect = None;
        state.search_rect = None;
        return;
    }

    let panes = if content.content.width >= 96 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(46), Constraint::Percentage(54)])
            .split(content.content)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(100), Constraint::Length(0)])
            .split(content.content)
    };
    render_resource_tree(buf, panes[0], state, &theme);
    state.preview_rect = (panes[1].width > 0).then_some(panes[1]);
    if panes[1].width > 0 {
        render_package_preview(buf, panes[1], state, &theme);
    }
}

fn render_resource_tree(buf: &mut Buffer, area: Rect, state: &mut PiConfigModalState, theme: &Theme) {
    let x = area.x;
    let width = area.width as usize;
    let mut y = area.y;
    let bottom = area.y.saturating_add(area.height);
    let scope_description = match (state.scope, state.catalog.project_trusted) {
        (PiResourceScope::User, true) => "Global · sources collapsed by default",
        (PiResourceScope::User, false) => "Global · project is not trusted",
        (PiResourceScope::Project, _) => "Project overrides · inherit/load/unload",
    };
    let search_label = if state.search_query.is_empty() {
        "Search resources…"
    } else {
        state.search_query.as_str()
    };
    let cursor = state.search_active.then_some("▌").unwrap_or("");
    write_line(
        buf,
        x,
        y,
        width,
        &format!("/ {search_label}{cursor}"),
        if state.search_active {
            Style::default().fg(theme.fuzzy_accent)
        } else {
            Style::default().fg(theme.gray_dim)
        },
    );
    state.search_rect = Some(Rect::new(x, y, area.width, 1));
    y = y.saturating_add(1);
    write_line(buf, x, y, width, scope_description, Style::default().fg(theme.gray_dim));
    y = y.saturating_add(1);
    if let Some(notice) = &state.notice {
        write_line(buf, x, y, width, notice, Style::default().fg(theme.fuzzy_accent));
        y = y.saturating_add(1);
    }

    let detail_y = bottom.saturating_sub(2);
    let help_y = bottom.saturating_sub(1);
    let list_height = detail_y.saturating_sub(y);
    state.list_rect = Some(Rect::new(x, y, area.width, list_height));
    state.list_viewport = list_height.max(1) as usize;
    state.clamp_selected();
    state.ensure_visible();

    let rows = state.visible_rows();
    if rows.is_empty() {
        write_line(buf, x, y, width, "No Pi resources match this scope or search.", Style::default().fg(theme.gray_dim));
    } else {
        let start = state.scroll.min(rows.len().saturating_sub(1));
        let end = (start + state.list_viewport).min(rows.len());
        for (offset, row) in rows[start..end].iter().enumerate() {
            let index = start + offset;
            let selected = index == state.selected;
            let style = if selected {
                Style::default().fg(theme.fuzzy_accent).bg(theme.bg_visual).add_modifier(Modifier::BOLD)
            } else if row.is_source() {
                Style::default().fg(theme.text_primary).add_modifier(Modifier::BOLD)
            } else if matches!(row, PiConfigRow::ResourceType { .. }) {
                Style::default().fg(theme.gray_dim).add_modifier(Modifier::ITALIC)
            } else {
                Style::default().fg(theme.gray_bright)
            };
            let text = match row {
                PiConfigRow::Source { id, label, resource_count, .. } => {
                    let fold = if state.folded_sources.contains(id) && state.search_query.is_empty() { "▸" } else { "▾" };
                    format!("{fold} {label} · {resource_count}")
                }
                PiConfigRow::ResourceType { label, .. } => format!("  {label}"),
                PiConfigRow::Resource(resource) => format!(
                    "    {} {}",
                    marker_for(resource, state.scope),
                    resource.display_name()
                ),
            };
            write_line(buf, x, y.saturating_add(offset as u16), width, &text, style);
        }
    }

    let count = rows.len();
    let position = if count == 0 { 0 } else { state.selected + 1 };
    let detail = rows.get(state.selected).map_or_else(
        || "No resource selected".to_owned(),
        |row| match row {
            PiConfigRow::Source { label, .. } => format!("{label} · Space/Enter toggles"),
            PiConfigRow::ResourceType { label, .. } => format!("{label} resources"),
            PiConfigRow::Resource(resource) => format!("{} · {}", resource.path.display(), resource.scope.label()),
        },
    );
    write_line(buf, x, detail_y, width, &detail, Style::default().fg(theme.gray_dim));
    let hint = if state.search_active { "Enter finish · Esc clear" } else { "click select · wheel scroll" };
    write_line(buf, x, help_y, width, &format!("({position}/{count}) {hint}"), Style::default().fg(theme.gray_dim));
}

fn render_package_preview(buf: &mut Buffer, area: Rect, state: &PiConfigModalState, theme: &Theme) {
    let preview = &state.preview;
    if area.width < 2 || area.height == 0 {
        return;
    }
    for row in area.y..area.y.saturating_add(area.height) {
        buf.set_string(area.x, row, "│", Style::default().fg(theme.gray_dim));
    }
    let x = area.x.saturating_add(2);
    let width = area.width.saturating_sub(2) as usize;
    let mut y = area.y;
    write_line(buf, x, y, width, "Package preview", Style::default().fg(theme.text_primary).add_modifier(Modifier::BOLD));
    y = y.saturating_add(1);
    write_line(buf, x, y, width, &preview.title, Style::default().fg(theme.fuzzy_accent));
    y = y.saturating_add(2);
    write_line(buf, x, y, width, "package.json", Style::default().fg(theme.text_primary).add_modifier(Modifier::BOLD));
    y = y.saturating_add(1);
    for line in &preview.manifest {
        if y >= area.y.saturating_add(area.height) {
            return;
        }
        write_line(buf, x, y, width, line, Style::default().fg(theme.gray_bright));
        y = y.saturating_add(1);
    }
    y = y.saturating_add(1);
    if y >= area.y.saturating_add(area.height) {
        return;
    }
    write_line(buf, x, y, width, "README", Style::default().fg(theme.text_primary).add_modifier(Modifier::BOLD));
    y = y.saturating_add(1);
    let markdown = MarkdownContent::new_with_table_width(&preview.readme, Some(width));
    for line in markdown.pre_wrap_lines().into_iter().skip(state.preview_scroll) {
        if y >= area.y.saturating_add(area.height) {
            return;
        }
        buf.set_line(x, y, &line, area.width.saturating_sub(2));
        y = y.saturating_add(1);
    }
}

fn marker_for(resource: &PiResource, scope: PiResourceScope) -> &'static str {
    if scope == PiResourceScope::Project {
        return match resource.project_override {
            PiProjectOverride::Load => "[+]",
            PiProjectOverride::Unload => "[-]",
            PiProjectOverride::Inherit if resource.inherited_enabled => "[x]",
            PiProjectOverride::Inherit => "[ ]",
        };
    }
    if resource.enabled { "[x]" } else { "[ ]" }
}

fn write_line(buf: &mut Buffer, x: u16, y: u16, width: usize, text: &str, style: Style) {
    buf.set_line(x, y, &Line::styled(text, style), width as u16);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi_resource_config::{PiResourceType, PiResourceOrigin};

    fn resource(name: &str, source: &str) -> PiResource {
        PiResource {
            path: PathBuf::from(format!("/tmp/pi/{name}")),
            resource_type: PiResourceType::Extensions,
            scope: PiResourceScope::User,
            origin: PiResourceOrigin::Auto,
            source: source.to_owned(),
            base_dir: PathBuf::from("/tmp/pi"),
            enabled: true,
            inherited_enabled: true,
            project_override: PiProjectOverride::Inherit,
        }
    }

    fn state() -> PiConfigModalState {
        let mut state = PiConfigModalState {
            window: ModalWindowState::new(),
            catalog: PiResourceCatalog {
                resources: vec![resource("extensions/alpha.ts", "auto"), resource("extensions/beta.ts", "auto")],
                project_trusted: true,
                agent_dir: PathBuf::from("/tmp/pi"),
                cwd: PathBuf::from("/tmp/project"),
            },
            scope: PiResourceScope::User,
            selected: 0,
            scroll: 0,
            folded_sources: HashSet::new(),
            search_query: String::new(),
            search_active: false,
            list_rect: None,
            search_rect: None,
            list_viewport: 4,
            preview_rect: None,
            preview_scroll: 0,
            preview: PackagePreview::default(),
            notice: None,
        };
        state.fold_all_sources();
        state.refresh_preview();
        state
    }

    #[test]
    fn project_cycle_preserves_pi_inherit_semantics() {
        assert_eq!(next_override(PiProjectOverride::Inherit, true), PiProjectOverride::Unload);
        assert_eq!(next_override(PiProjectOverride::Unload, true), PiProjectOverride::Load);
        assert_eq!(next_override(PiProjectOverride::Load, true), PiProjectOverride::Inherit);
        assert_eq!(next_override(PiProjectOverride::Inherit, false), PiProjectOverride::Load);
    }

    #[test]
    fn sources_are_collapsed_by_default_and_search_expands_matches() {
        let mut state = state();
        assert_eq!(state.visible_rows().len(), 1);
        state.search_query = "alpha".to_owned();
        assert_eq!(state.visible_rows().len(), 3);
    }

    #[test]
    fn automatic_resource_names_keep_the_relative_path() {
        assert_eq!(resource("extensions/alpha.ts", "auto").display_name(), "extensions/alpha.ts");
    }

    #[test]
    fn github_source_has_a_concise_identity_label() {
        let mut resource = resource("index.ts", "https://github.com/acme/example.git");
        resource.origin = PiResourceOrigin::Package;
        assert_eq!(source_label(&resource), "GitHub · acme/example");
    }

    #[test]
    fn mouse_click_on_source_header_folds_or_expands_it() {
        let mut state = state();
        state.list_rect = Some(Rect::new(0, 0, 60, 4));
        let _ = state.handle_mouse(&MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 4,
            row: 0,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
        assert_eq!(state.visible_rows().len(), 4);
    }

    #[test]
    fn preview_reads_manifest_and_readme() {
        let temp = tempfile::tempdir().expect("temp directory");
        fs::write(temp.path().join("package.json"), r#"{"name":"demo","version":"1.0.0"}"#).expect("manifest");
        fs::write(temp.path().join("README.md"), "# Demo\n\nUseful package\n").expect("readme");
        let mut resource = resource("index.ts", "npm:demo");
        resource.origin = PiResourceOrigin::Package;
        resource.base_dir = temp.path().to_path_buf();
        let preview = package_preview(resource, "demo".to_owned());
        assert!(preview.manifest.iter().any(|line| line == "name: demo"));
        assert!(preview.readme.contains("# Demo"));
    }
}
