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
    PiResourceType,
};
use crate::pi_resource_policy::ResourcePolicy;
use crate::scrollback::blocks::markdown_content::MarkdownContent;
use crate::theme::Theme;
use crate::views::modal_window::{
    ModalSizing, ModalWindowConfig, ModalWindowState, Shortcut, render_modal_window,
};

const TABS: [&str; 2] = ["Global", "Project"];

/// Mirrors the enabled-state filtering used by Grok's native MCP/plugin modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ResourceFilter {
    #[default]
    All,
    Enabled,
    Disabled,
}

impl ResourceFilter {
    const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Enabled => "Enabled",
            Self::Disabled => "Disabled",
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::All => Self::Enabled,
            Self::Enabled => Self::Disabled,
            Self::Disabled => Self::All,
        }
    }

    const fn matches(self, enabled: bool) -> bool {
        match self {
            Self::All => true,
            Self::Enabled => enabled,
            Self::Disabled => !enabled,
        }
    }
}

const SHORTCUTS: [Shortcut<'static>; 9] = [
    Shortcut {
        label: "↑/↓ navigate",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "←/→ fold",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "Space toggle",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "f filter",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "a policy",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "/ search",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "Tab/⇧Tab scope",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "r refresh",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "Esc close",
        clickable: false,
        id: 0,
    },
];

const PICKER_SHORTCUTS: [Shortcut<'static>; 8] = [
    Shortcut {
        label: "↑/↓ navigate",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "←/→ fold",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "Space select",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "Enter apply",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "/ search",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "Tab/⇧Tab scope",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "r refresh",
        clickable: false,
        id: 0,
    },
    Shortcut {
        label: "Esc cancel",
        clickable: false,
        id: 0,
    },
];

type PickerResponseTx =
    tokio::sync::oneshot::Sender<xai_acp_lib::AcpResult<agent_client_protocol::ExtResponse>>;

/// A product-owned selection request rendered with the existing Pi resource
/// manager. It does not mutate Pi's global/project resource enablement.
pub struct PiResourcePickerRequest {
    pub title: String,
    pub resource_types: Vec<PiResourceType>,
    pub initial_paths: Vec<PathBuf>,
    pub extra_resources: Vec<PiResourcePickerExtra>,
}

/// A transient grok-pi bridge resource (typically an injected extension) that
/// Pi's durable settings catalog cannot discover by itself.
pub struct PiResourcePickerExtra {
    pub path: PathBuf,
    pub label: String,
    pub resource_type: PiResourceType,
}

struct PiResourcePickerState {
    title: String,
    resource_types: HashSet<PiResourceType>,
    selected_paths: HashSet<PathBuf>,
    extra_resources: Vec<PiResource>,
    response_tx: Option<PickerResponseTx>,
}

#[derive(Clone)]
enum PiConfigRow {
    Source {
        id: String,
        label: String,
        resource_count: usize,
        preview: PiResource,
    },
    Root {
        id: String,
        label: String,
        resource_count: usize,
        preview: PiResource,
    },
    ResourceType {
        id: String,
        label: String,
        depth: usize,
        preview: PiResource,
    },
    Resource(PiResource),
}

impl PiConfigRow {
    fn is_group(&self) -> bool {
        matches!(
            self,
            Self::Root { .. } | Self::Source { .. } | Self::ResourceType { .. }
        )
    }

    fn group_id(&self) -> Option<&str> {
        match self {
            Self::Root { id, .. } | Self::Source { id, .. } | Self::ResourceType { id, .. } => {
                Some(id)
            }
            Self::Resource(_) => None,
        }
    }

    fn preview_resource(&self) -> &PiResource {
        match self {
            Self::Root { preview, .. }
            | Self::Source { preview, .. }
            | Self::ResourceType { preview, .. }
            | Self::Resource(preview) => preview,
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
    policy: ResourcePolicy,
    scope: PiResourceScope,
    selected: usize,
    scroll: usize,
    folded_sources: HashSet<String>,
    filter: ResourceFilter,
    search_query: String,
    search_active: bool,
    list_rect: Option<Rect>,
    search_rect: Option<Rect>,
    list_viewport: usize,
    preview_rect: Option<Rect>,
    preview_scroll: usize,
    preview: PackagePreview,
    notice: Option<String>,
    picker: Option<PiResourcePickerState>,
}

pub enum PiConfigOutcome {
    Close,
    PickerSubmit,
    Changed,
}

impl PiConfigModalState {
    pub fn open(cwd: PathBuf) -> Result<Self> {
        let catalog = PiResourceCatalog::load(cwd)?;
        let policy = ResourcePolicy::load_from_config();
        let mut state = Self {
            window: ModalWindowState::new(),
            catalog,
            policy,
            scope: PiResourceScope::User,
            selected: 0,
            scroll: 0,
            folded_sources: HashSet::new(),
            filter: ResourceFilter::default(),
            search_query: String::new(),
            search_active: false,
            list_rect: None,
            search_rect: None,
            list_viewport: 1,
            preview_rect: None,
            preview_scroll: 0,
            preview: PackagePreview::default(),
            notice: None,
            picker: None,
        };
        state.fold_all_sources();
        state.refresh_preview();
        Ok(state)
    }

    /// Open the normal Pi resource manager in multi-select mode for a
    /// subagent definition. Selection lives only in the caller's Markdown;
    /// Space never changes Pi's own resource enablement in this mode.
    pub fn open_picker(
        cwd: PathBuf,
        request: PiResourcePickerRequest,
        response_tx: PickerResponseTx,
    ) -> Result<Self> {
        let mut state = Self::open(cwd)?;
        let extra_resources = request
            .extra_resources
            .into_iter()
            .map(|extra| PiResource {
                path: extra.path.clone(),
                resource_type: extra.resource_type,
                scope: PiResourceScope::User,
                origin: PiResourceOrigin::Settings,
                source: format!("grok-pi:{}", extra.label),
                base_dir: extra
                    .path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from(".")),
                enabled: true,
                inherited_enabled: true,
                project_override: PiProjectOverride::Inherit,
            })
            .collect::<Vec<_>>();
        let selected_paths = request.initial_paths.into_iter().collect();
        state.picker = Some(PiResourcePickerState {
            title: request.title,
            resource_types: request.resource_types.into_iter().collect(),
            selected_paths,
            extra_resources,
            response_tx: Some(response_tx),
        });
        state.merge_picker_resources();
        state.fold_all_sources();
        state.refresh_preview();
        Ok(state)
    }

    pub fn complete_picker(&mut self, accepted: bool) {
        let Some(mut picker) = self.picker.take() else {
            return;
        };
        let payload = if accepted {
            let mut paths = picker
                .selected_paths
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            paths.sort();
            serde_json::json!({ "outcome": "accepted", "paths": paths })
        } else {
            serde_json::json!({ "outcome": "cancelled" })
        };
        let raw = serde_json::value::to_raw_value(&payload)
            .expect("Pi resource picker response should be serializable");
        if let Some(tx) = picker.response_tx.take() {
            let _ = tx.send(Ok(agent_client_protocol::ExtResponse::new(raw.into())));
        }
    }

    pub fn is_picker(&self) -> bool {
        self.picker.is_some()
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
            KeyCode::Tab | KeyCode::BackTab if self.catalog.project_trusted => {
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
            KeyCode::Enter if key.modifiers.is_empty() && self.is_picker() => {
                PiConfigOutcome::PickerSubmit
            }
            KeyCode::Char(' ') | KeyCode::Enter if key.modifiers.is_empty() => {
                self.activate_selected();
                PiConfigOutcome::Changed
            }
            KeyCode::Char('r') if key.modifiers.is_empty() => {
                self.refresh();
                PiConfigOutcome::Changed
            }
            KeyCode::Char('f') if key.modifiers.is_empty() => {
                self.filter = self.filter.next();
                self.reset_after_filter_change();
                PiConfigOutcome::Changed
            }
            KeyCode::Char('a') if key.modifiers.is_empty() => {
                if !self.is_picker() {
                    self.toggle_policy();
                }
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
                // List interaction leaves search (F2/picker parity); keep query.
                self.search_active = false;
                self.move_selection(-3);
                PiConfigOutcome::Changed
            }
            MouseEventKind::ScrollDown => {
                self.search_active = false;
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
                // Clicking the tree is browse mode — Space/j/k must not type.
                self.search_active = false;
                let Some(index) = self.hit_test_row(mouse.column, mouse.row) else {
                    return PiConfigOutcome::Changed;
                };
                let was_selected = self.selected == index;
                self.selected = index;
                self.ensure_visible();
                if self
                    .visible_rows()
                    .get(index)
                    .is_some_and(PiConfigRow::is_group)
                {
                    self.toggle_selected_group();
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
        // Nav / toggle leave search focus and re-dispatch (picker + F2 pattern).
        // Keep query so the filtered list stays; only Esc clears it.
        if Self::search_exit_nav_key(key) {
            self.search_active = false;
            return self.handle_key(key);
        }
        match key.code {
            KeyCode::Tab | KeyCode::BackTab if self.catalog.project_trusted => {
                // Keep the query so users can compare the same match across
                // Global and Project without first dismissing search.
                self.search_active = false;
                self.select_tab(match self.scope {
                    PiResourceScope::User => 1,
                    PiResourceScope::Project => 0,
                });
            }
            KeyCode::Esc => {
                if self.search_query.is_empty() {
                    self.search_active = false;
                } else {
                    self.search_query.clear();
                    self.reset_after_filter_change();
                }
            }
            // Commit filter → list focus (preserve query for Space toggle).
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

    /// Arrow/page nav leaves search (picker parity). Char keys still type.
    fn search_exit_nav_key(key: &KeyEvent) -> bool {
        matches!(
            key.code,
            KeyCode::Up
                | KeyCode::Down
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::Left
                | KeyCode::Right
        )
    }

    fn visible_rows(&self) -> Vec<PiConfigRow> {
        let mut roots: BTreeMap<String, (String, Vec<PiResource>)> = BTreeMap::new();
        for resource in self.catalog.resources_for_scope(self.scope) {
            if !self.matches_resource(resource) {
                continue;
            }
            let (id, label) = root_group(resource);
            roots
                .entry(id)
                .or_insert_with(|| (label, Vec::new()))
                .1
                .push(resource.clone());
        }

        let searching = !self.search_query.trim().is_empty();
        let mut rows = Vec::new();
        for (root_id, (root_label, mut resources)) in roots {
            resources.sort_by_cached_key(|resource| {
                (
                    resource.resource_type,
                    resource.source.clone(),
                    resource.display_name().to_lowercase(),
                )
            });
            let preview = resources[0].clone();
            rows.push(PiConfigRow::Root {
                id: root_id.clone(),
                label: root_label,
                resource_count: resources.len(),
                preview,
            });
            if !searching && self.folded_sources.contains(&root_id) {
                continue;
            }

            if resources
                .iter()
                .all(|resource| resource.origin == PiResourceOrigin::Package)
            {
                let mut packages: BTreeMap<String, (String, Vec<PiResource>)> = BTreeMap::new();
                for resource in resources {
                    let id = source_id(&resource);
                    packages
                        .entry(id)
                        .or_insert_with(|| (source_label(&resource), Vec::new()))
                        .1
                        .push(resource);
                }
                for (source, (label, package_resources)) in packages {
                    let preview = package_resources[0].clone();
                    rows.push(PiConfigRow::Source {
                        id: source.clone(),
                        label,
                        resource_count: package_resources.len(),
                        preview,
                    });
                    append_type_rows(
                        &mut rows,
                        &self.folded_sources,
                        searching,
                        &source,
                        package_resources,
                        2,
                    );
                }
            } else {
                append_type_rows(
                    &mut rows,
                    &self.folded_sources,
                    searching,
                    &root_id,
                    resources,
                    1,
                );
            }
        }
        rows
    }

    fn source_ids(&self) -> HashSet<String> {
        self.catalog
            .resources_for_scope(self.scope)
            .into_iter()
            .flat_map(|resource| {
                let (root_id, _) = root_group(resource);
                let mut ids = vec![root_id];
                if resource.origin == PiResourceOrigin::Package {
                    ids.push(source_id(resource));
                }
                ids.push(type_group_id(&source_id(resource), resource.resource_type));
                ids
            })
            .collect()
    }

    fn fold_all_sources(&mut self) {
        self.folded_sources.extend(self.source_ids());
        self.clamp_selected();
    }

    fn matches_resource(&self, resource: &PiResource) -> bool {
        if let Some(picker) = &self.picker
            && !picker.resource_types.contains(&resource.resource_type)
        {
            return false;
        }
        if !self.filter.matches(resource.enabled) {
            return false;
        }
        let query = self.search_query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        let policy_key = policy_key(resource);
        let policy_state = if self.policy.allow.iter().any(|entry| entry == &policy_key) {
            "policy allowed"
        } else if self.policy.block.iter().any(|entry| entry == &policy_key) {
            "policy blocked"
        } else if crate::pi_resource_policy::DEFAULT_BLOCKED_SOURCES
            .iter()
            .any(|source| *source == resource.source)
        {
            "policy default blocked"
        } else if resource.enabled {
            "enabled"
        } else {
            "disabled"
        };
        [
            resource.display_name(),
            resource.resource_type.label().to_owned(),
            source_label(resource),
            resource.path.display().to_string(),
            policy_state.to_owned(),
        ]
        .into_iter()
        .any(|value| value.to_lowercase().contains(&query))
    }

    fn scope_resource_count(&self) -> usize {
        self.catalog
            .resources_for_scope(self.scope)
            .into_iter()
            .filter(|resource| {
                self.picker
                    .as_ref()
                    .is_none_or(|picker| picker.resource_types.contains(&resource.resource_type))
            })
            .count()
    }

    fn matching_resource_count(&self) -> usize {
        self.catalog
            .resources_for_scope(self.scope)
            .into_iter()
            .filter(|resource| self.matches_resource(resource))
            .count()
    }

    fn activate_selected(&mut self) {
        if self
            .visible_rows()
            .get(self.selected)
            .is_some_and(PiConfigRow::is_group)
        {
            self.toggle_selected_group();
        } else {
            self.toggle_selected_resource();
        }
    }

    fn toggle_selected_group(&mut self) {
        let Some(id) = self
            .visible_rows()
            .get(self.selected)
            .and_then(PiConfigRow::group_id)
            .map(str::to_owned)
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
        let Some(id) = self
            .visible_rows()
            .get(self.selected)
            .and_then(PiConfigRow::group_id)
            .map(str::to_owned)
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
        if let Some(picker) = &mut self.picker {
            if !picker.selected_paths.insert(resource.path.clone()) {
                picker.selected_paths.remove(&resource.path);
            }
            self.notice = Some(format!(
                "{} selected for this subagent",
                resource.display_name()
            ));
            return;
        }
        let result = match self.scope {
            PiResourceScope::User => self
                .catalog
                .set_global_enabled(&resource, !resource.enabled),
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

    /// Toggle the grok-pi resource policy (allow/block) for the selected
    /// resource.  Cycles: default → block → allow → default.  Persists to
    /// `~/.grok/config.toml` under `[pi.resources]`.
    fn toggle_policy(&mut self) {
        let Some(PiConfigRow::Resource(resource)) = self.visible_rows().get(self.selected).cloned()
        else {
            return;
        };
        let key = policy_key(&resource);
        let in_allow = self.policy.allow.iter().any(|entry| entry == &key);
        let in_block = self.policy.block.iter().any(|entry| entry == &key);
        if in_allow {
            // allow → default (remove from allow)
            self.policy.allow.retain(|entry| entry != &key);
            self.notice = Some(format!(
                "Policy: removed allow for {}",
                resource.display_name()
            ));
        } else if in_block {
            // block → allow
            self.policy.block.retain(|entry| entry != &key);
            self.policy.allow.push(key.clone());
            self.notice = Some(format!(
                "Policy: {} → allow (overrides default blocklist)",
                resource.display_name()
            ));
        } else {
            // default → block
            self.policy.block.push(key);
            self.notice = Some(format!("Policy: {} → block", resource.display_name()));
        }
        if let Err(error) = self.policy.save_to_config() {
            self.notice = Some(format!("Failed to save policy: {error}"));
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
        self.selected = self
            .selected
            .min(self.visible_rows().len().saturating_sub(1));
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
        self.policy = ResourcePolicy::load_from_config();
        match PiResourceCatalog::load(self.catalog.cwd.clone()) {
            Ok(catalog) => {
                self.catalog = catalog;
                self.merge_picker_resources();
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
        let key = format!(
            "{}:{}",
            resource.base_dir.display(),
            resource.path.display()
        );
        if self.preview.key == key {
            return;
        }
        self.preview_scroll = 0;
        self.preview = package_preview(resource, key);
    }

    fn merge_picker_resources(&mut self) {
        let Some(picker) = &self.picker else {
            return;
        };
        for extra in &picker.extra_resources {
            if !self.catalog.resources.iter().any(|resource| {
                resource.path == extra.path && resource.resource_type == extra.resource_type
            }) {
                self.catalog.resources.push(extra.clone());
            }
        }
    }

    fn picker_selected(&self, resource: &PiResource) -> bool {
        self.picker
            .as_ref()
            .is_some_and(|picker| picker.selected_paths.contains(&resource.path))
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

fn policy_key(resource: &PiResource) -> String {
    if resource.origin == PiResourceOrigin::Package {
        resource.source.clone()
    } else {
        resource.path.to_string_lossy().into_owned()
    }
}

fn root_group(resource: &PiResource) -> (String, String) {
    match resource.origin {
        PiResourceOrigin::Auto if resource.path.starts_with(&resource.base_dir) => {
            ("auto".to_owned(), "Auto-discovered".to_owned())
        }
        PiResourceOrigin::Auto => ("manual".to_owned(), "Manual paths".to_owned()),
        PiResourceOrigin::Settings => ("settings".to_owned(), "Settings paths".to_owned()),
        PiResourceOrigin::Package if github_repo(&resource.source).is_some() => {
            ("github".to_owned(), "GitHub".to_owned())
        }
        PiResourceOrigin::Package if resource.source.starts_with("npm:") => {
            ("npm".to_owned(), "npm".to_owned())
        }
        PiResourceOrigin::Package => ("packages".to_owned(), "Packages".to_owned()),
    }
}

fn type_group_id(parent: &str, resource_type: crate::pi_resource_config::PiResourceType) -> String {
    format!("{parent}:type:{:?}", resource_type)
}

fn append_type_rows(
    rows: &mut Vec<PiConfigRow>,
    folded: &HashSet<String>,
    searching: bool,
    parent_id: &str,
    resources: Vec<PiResource>,
    depth: usize,
) {
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
        let Some(preview) = typed.first().cloned() else {
            continue;
        };
        let id = type_group_id(parent_id, resource_type);
        rows.push(PiConfigRow::ResourceType {
            id: id.clone(),
            label: resource_type.label().to_owned(),
            depth,
            preview,
        });
        if !searching && folded.contains(&id) {
            continue;
        }
        rows.extend(typed.into_iter().map(PiConfigRow::Resource));
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
        PiResourceOrigin::Package if github_repo(&resource.source).is_some() => {
            format!(
                "GitHub · {}",
                github_repo(&resource.source).unwrap_or_default()
            )
        }
        PiResourceOrigin::Package if resource.source.starts_with("npm:") => {
            format!("npm · {}", resource.source.trim_start_matches("npm:"))
        }
        PiResourceOrigin::Package => format!("Package · {}", resource.source),
        PiResourceOrigin::Auto => format!("Auto-discovered · {}", resource.base_dir.display()),
        PiResourceOrigin::Settings => format!("Settings path · {}", resource.base_dir.display()),
    }
}

fn github_repo(source: &str) -> Option<&str> {
    source
        .strip_prefix("git:github.com/")
        .or_else(|| source.strip_prefix("https://github.com/"))
        .or_else(|| source.strip_prefix("http://github.com/"))
        .map(|repo| repo.trim_end_matches(".git"))
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
    let (title, shortcuts) = if let Some(picker) = &state.picker {
        (picker.title.as_str(), &PICKER_SHORTCUTS[..])
    } else {
        ("Pi resources", &SHORTCUTS[..])
    };
    let config = ModalWindowConfig {
        title,
        tabs: state.catalog.project_trusted.then_some(&TABS),
        shortcuts,
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

fn render_resource_tree(
    buf: &mut Buffer,
    area: Rect,
    state: &mut PiConfigModalState,
    theme: &Theme,
) {
    let x = area.x;
    let width = area.width as usize;
    let mut y = area.y;
    let bottom = area.y.saturating_add(area.height);
    let scope_description = match (
        state.scope,
        state.catalog.project_trusted,
        state.is_picker(),
    ) {
        (_, _, true) => "Selection only · Pi resource settings are unchanged",
        (PiResourceScope::User, true, false) => "Global · sources collapsed by default",
        (PiResourceScope::User, false, false) => "Global · project is not trusted",
        (PiResourceScope::Project, _, false) => "Project overrides · inherit/load/unload",
    };
    let matching_resources = state.matching_resource_count();
    let scope_resources = state.scope_resource_count();
    let scope_description = format!(
        "{scope_description} · {} · {matching_resources}/{scope_resources} resources",
        state.filter.label(),
    );
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
        &format!("/ [{}] {search_label}{cursor}", state.filter.label()),
        if state.search_active {
            Style::default().fg(theme.fuzzy_accent)
        } else {
            Style::default().fg(theme.gray_dim)
        },
    );
    state.search_rect = Some(Rect::new(x, y, area.width, 1));
    y = y.saturating_add(1);
    write_line(
        buf,
        x,
        y,
        width,
        &scope_description,
        Style::default().fg(theme.gray_dim),
    );
    y = y.saturating_add(1);
    if let Some(notice) = &state.notice {
        write_line(
            buf,
            x,
            y,
            width,
            notice,
            Style::default().fg(theme.fuzzy_accent),
        );
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
        write_line(
            buf,
            x,
            y,
            width,
            "No Pi resources match this scope or search.",
            Style::default().fg(theme.gray_dim),
        );
    } else {
        let start = state.scroll.min(rows.len().saturating_sub(1));
        let end = (start + state.list_viewport).min(rows.len());
        for (offset, row) in rows[start..end].iter().enumerate() {
            let index = start + offset;
            let selected = index == state.selected;
            let style = if selected {
                Style::default()
                    .fg(theme.fuzzy_accent)
                    .bg(theme.bg_visual)
                    .add_modifier(Modifier::BOLD)
            } else if matches!(row, PiConfigRow::Root { .. } | PiConfigRow::Source { .. }) {
                Style::default()
                    .fg(theme.text_primary)
                    .add_modifier(Modifier::BOLD)
            } else if matches!(row, PiConfigRow::ResourceType { .. }) {
                Style::default()
                    .fg(theme.gray_dim)
                    .add_modifier(Modifier::ITALIC)
            } else {
                Style::default().fg(theme.gray_bright)
            };
            let text = match row {
                PiConfigRow::Root {
                    id,
                    label,
                    resource_count,
                    ..
                }
                | PiConfigRow::Source {
                    id,
                    label,
                    resource_count,
                    ..
                } => {
                    let fold = if state.folded_sources.contains(id) && state.search_query.is_empty()
                    {
                        "▸"
                    } else {
                        "▾"
                    };
                    let indent = matches!(row, PiConfigRow::Source { .. });
                    format!(
                        "{} {fold} {label} · {resource_count}",
                        if indent { "  " } else { "" }
                    )
                }
                PiConfigRow::ResourceType {
                    id, label, depth, ..
                } => {
                    let fold = if state.folded_sources.contains(id) && state.search_query.is_empty()
                    {
                        "▸"
                    } else {
                        "▾"
                    };
                    format!("{} {fold} {label}", "  ".repeat(*depth))
                }
                PiConfigRow::Resource(resource) => {
                    let policy_tag = policy_marker(&state.policy, resource);
                    format!(
                        "      {} {}{}",
                        if state.is_picker() {
                            picker_marker(state.picker_selected(resource))
                        } else {
                            marker_for(resource, state.scope)
                        },
                        resource.display_name(),
                        policy_tag,
                    )
                }
            };
            write_line(buf, x, y.saturating_add(offset as u16), width, &text, style);
        }
    }

    let count = rows.len();
    let position = if count == 0 { 0 } else { state.selected + 1 };
    let detail = rows.get(state.selected).map_or_else(
        || "No resource selected".to_owned(),
        |row| match row {
            PiConfigRow::Root { label, .. } | PiConfigRow::Source { label, .. } => {
                if state.is_picker() {
                    format!("{label} · Space expands or collapses")
                } else {
                    format!("{label} · Space/Enter toggles")
                }
            }
            PiConfigRow::ResourceType { label, .. } => {
                if state.is_picker() {
                    format!("{label} · Space expands or collapses")
                } else {
                    format!("{label} · Space/Enter toggles")
                }
            }
            PiConfigRow::Resource(resource) => {
                if state.is_picker() {
                    format!(
                        "{} · {} · Space select · Enter apply",
                        resource.path.display(),
                        if state.picker_selected(resource) {
                            "selected"
                        } else {
                            "not selected"
                        },
                    )
                } else {
                    let key = policy_key(resource);
                    let pol = if state.policy.allow.iter().any(|entry| entry == &key) {
                        " · policy: allow"
                    } else if state.policy.block.iter().any(|entry| entry == &key) {
                        " · policy: block"
                    } else {
                        ""
                    };
                    format!(
                        "{} · {}{pol}",
                        resource.path.display(),
                        resource.scope.label()
                    )
                }
            }
        },
    );
    write_line(
        buf,
        x,
        detail_y,
        width,
        &detail,
        Style::default().fg(theme.gray_dim),
    );
    let hint = if state.search_active {
        "Enter finish · Esc clear"
    } else if state.is_picker() {
        "Space select · Enter apply · Esc cancel"
    } else {
        "click select · wheel scroll"
    };
    write_line(
        buf,
        x,
        help_y,
        width,
        &format!(
            "({position}/{count}) {} · {matching_resources}/{scope_resources} resources · {hint}",
            state.filter.label(),
        ),
        Style::default().fg(theme.gray_dim),
    );
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
    write_line(
        buf,
        x,
        y,
        width,
        "Package preview",
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    );
    y = y.saturating_add(1);
    write_line(
        buf,
        x,
        y,
        width,
        &preview.title,
        Style::default().fg(theme.fuzzy_accent),
    );
    y = y.saturating_add(2);
    write_line(
        buf,
        x,
        y,
        width,
        "package.json",
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    );
    y = y.saturating_add(1);
    for line in &preview.manifest {
        if y >= area.y.saturating_add(area.height) {
            return;
        }
        write_line(
            buf,
            x,
            y,
            width,
            line,
            Style::default().fg(theme.gray_bright),
        );
        y = y.saturating_add(1);
    }
    y = y.saturating_add(1);
    if y >= area.y.saturating_add(area.height) {
        return;
    }
    write_line(
        buf,
        x,
        y,
        width,
        "README",
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    );
    y = y.saturating_add(1);
    let markdown = MarkdownContent::new_with_table_width(&preview.readme, Some(width));
    for line in markdown
        .pre_wrap_lines()
        .into_iter()
        .skip(state.preview_scroll)
    {
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

fn picker_marker(selected: bool) -> &'static str {
    if selected { "[x]" } else { "[ ]" }
}

/// Policy tag shown next to a resource name in the tree.
/// Returns a short suffix like " ⛔ blocked" or " ✅ forced" or empty string.
fn policy_marker(policy: &ResourcePolicy, resource: &PiResource) -> String {
    let source = policy_key(resource);
    if policy.allow.iter().any(|entry| entry == &source) {
        return " ✅ forced".to_owned();
    }
    if policy.block.iter().any(|entry| entry == &source) {
        return " ⛔ blocked".to_owned();
    }
    // Check default blocklist by package source.
    for blocked in crate::pi_resource_policy::DEFAULT_BLOCKED_SOURCES {
        if resource.source == *blocked {
            return " ⛔ default-blocked".to_owned();
        }
    }
    String::new()
}

fn write_line(buf: &mut Buffer, x: u16, y: u16, width: usize, text: &str, style: Style) {
    buf.set_line(x, y, &Line::styled(text, style), width as u16);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi_resource_config::{PiResourceOrigin, PiResourceType};

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
                resources: vec![
                    resource("extensions/alpha.ts", "auto"),
                    resource("extensions/beta.ts", "auto"),
                ],
                project_trusted: true,
                agent_dir: PathBuf::from("/tmp/pi"),
                cwd: PathBuf::from("/tmp/project"),
            },
            policy: ResourcePolicy::default(),
            scope: PiResourceScope::User,
            selected: 0,
            scroll: 0,
            folded_sources: HashSet::new(),
            filter: ResourceFilter::default(),
            search_query: String::new(),
            search_active: false,
            list_rect: None,
            search_rect: None,
            list_viewport: 4,
            preview_rect: None,
            preview_scroll: 0,
            preview: PackagePreview::default(),
            notice: None,
            picker: None,
        };
        state.fold_all_sources();
        state.refresh_preview();
        state
    }

    #[test]
    fn project_cycle_preserves_pi_inherit_semantics() {
        assert_eq!(
            next_override(PiProjectOverride::Inherit, true),
            PiProjectOverride::Unload
        );
        assert_eq!(
            next_override(PiProjectOverride::Unload, true),
            PiProjectOverride::Load
        );
        assert_eq!(
            next_override(PiProjectOverride::Load, true),
            PiProjectOverride::Inherit
        );
        assert_eq!(
            next_override(PiProjectOverride::Inherit, false),
            PiProjectOverride::Load
        );
    }

    #[test]
    fn sources_are_collapsed_by_default_and_search_expands_matches() {
        let mut state = state();
        assert_eq!(state.visible_rows().len(), 1);
        state.search_query = "alpha".to_owned();
        assert_eq!(state.visible_rows().len(), 3);
    }

    #[test]
    fn status_filter_cycles_and_limits_visible_resources() {
        let mut state = state();
        state.catalog.resources[1].enabled = false;

        let key = KeyEvent::new(KeyCode::Char('f'), crossterm::event::KeyModifiers::NONE);
        state.handle_key(&key);
        assert_eq!(state.filter, ResourceFilter::Enabled);
        assert_eq!(state.matching_resource_count(), 1);

        state.handle_key(&key);
        assert_eq!(state.filter, ResourceFilter::Disabled);
        assert_eq!(state.matching_resource_count(), 1);

        state.handle_key(&key);
        assert_eq!(state.filter, ResourceFilter::All);
        assert_eq!(state.matching_resource_count(), 2);
    }

    #[test]
    fn picker_toggles_its_local_selection_without_changing_pi_enablement() {
        let mut state = state();
        let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
        state.picker = Some(PiResourcePickerState {
            title: "Subagent extensions".to_owned(),
            resource_types: [PiResourceType::Extensions].into_iter().collect(),
            selected_paths: HashSet::new(),
            extra_resources: Vec::new(),
            response_tx: Some(response_tx),
        });
        // Search expands the source/type tree: root, type, then alpha.
        state.search_query = "alpha".to_owned();
        state.selected = 2;
        state.toggle_selected_resource();

        let path = state.catalog.resources[0].path.clone();
        assert!(state.picker_selected(&state.catalog.resources[0]));
        assert!(
            state.catalog.resources[0].enabled,
            "picker must not mutate Pi settings"
        );
        assert_eq!(
            state.picker.as_ref().unwrap().selected_paths,
            HashSet::from([path])
        );
    }

    #[test]
    fn tab_and_backtab_switch_scope_without_clearing_search() {
        let mut state = state();
        state.search_query = "alpha".to_owned();
        let tab = KeyEvent::new(KeyCode::Tab, crossterm::event::KeyModifiers::NONE);
        let backtab = KeyEvent::new(KeyCode::BackTab, crossterm::event::KeyModifiers::SHIFT);

        state.handle_key(&tab);
        assert_eq!(state.scope, PiResourceScope::Project);
        assert_eq!(state.search_query, "alpha");

        state.search_active = true;
        state.handle_key(&backtab);
        assert_eq!(state.scope, PiResourceScope::User);
        assert_eq!(state.search_query, "alpha");
        assert!(!state.search_active);
    }

    #[test]
    fn search_active_arrow_keys_exit_to_list_nav() {
        let mut state = state();
        state.search_query = "alpha".to_owned();
        state.search_active = true;

        let down = KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE);
        state.handle_key(&down);
        assert!(!state.search_active, "Down leaves search focus");
        assert_eq!(state.search_query, "alpha", "query preserved");

        // Space while still searching types into the query (not toggle).
        state.search_active = true;
        let space = KeyEvent::new(KeyCode::Char(' '), crossterm::event::KeyModifiers::NONE);
        state.handle_key(&space);
        assert!(state.search_active);
        assert!(state.search_query.ends_with(' '));
    }

    #[test]
    fn mouse_list_click_blurs_search_so_space_is_list_action() {
        let mut state = state();
        state.search_active = true;
        state.search_query = "alpha".to_owned();
        state.list_rect = Some(Rect::new(0, 2, 40, 10));
        state.list_viewport = 10;
        state.search_rect = Some(Rect::new(0, 0, 40, 1));

        let resource_idx = state
            .visible_rows()
            .iter()
            .position(|row| matches!(row, PiConfigRow::Resource(_)))
            .expect("search expands matching resource rows");

        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 2 + resource_idx as u16,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        state.handle_mouse(&mouse);
        assert!(!state.search_active, "list click must blur search");
        assert_eq!(state.search_query, "alpha");
        assert_eq!(state.selected, resource_idx);

        // After blur, Space is list nav (not query append). Do not call
        // real toggle — set_global_enabled writes disk.
        let space = KeyEvent::new(KeyCode::Char(' '), crossterm::event::KeyModifiers::NONE);
        let query_before = state.search_query.clone();
        state.handle_key(&space);
        assert!(!state.search_active);
        assert_eq!(
            state.search_query, query_before,
            "Space must not type after list focus"
        );
    }

    #[test]
    fn automatic_resource_names_keep_the_relative_path() {
        assert_eq!(
            resource("extensions/alpha.ts", "auto").display_name(),
            "extensions/alpha.ts"
        );
    }

    #[test]
    fn policy_key_is_unique_for_auto_discovered_resources() {
        let alpha = resource("extensions/alpha.ts", "auto");
        let beta = resource("extensions/beta.ts", "auto");
        assert_ne!(policy_key(&alpha), policy_key(&beta));
        assert_eq!(policy_key(&alpha), "/tmp/pi/extensions/alpha.ts");
    }

    #[test]
    fn github_source_has_a_concise_identity_label() {
        let mut resource = resource("index.ts", "https://github.com/acme/example.git");
        resource.origin = PiResourceOrigin::Package;
        assert_eq!(source_label(&resource), "GitHub · acme/example");
        resource.source = "git:github.com/acme/example".to_owned();
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
        fs::write(
            temp.path().join("package.json"),
            r#"{"name":"demo","version":"1.0.0"}"#,
        )
        .expect("manifest");
        fs::write(temp.path().join("README.md"), "# Demo\n\nUseful package\n").expect("readme");
        let mut resource = resource("index.ts", "npm:demo");
        resource.origin = PiResourceOrigin::Package;
        resource.base_dir = temp.path().to_path_buf();
        let preview = package_preview(resource, "demo".to_owned());
        assert!(preview.manifest.iter().any(|line| line == "name: demo"));
        assert!(preview.readme.contains("# Demo"));
    }
}
