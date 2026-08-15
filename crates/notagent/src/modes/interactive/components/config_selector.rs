//! 1:1 port of
//! `packages/coding-agent/src/modes/interactive/components/config-selector.ts` (942 LOC).
//!
//! The resource browser of `notagent config`: it groups every skill, prompt
//! template and theme by where it came from and writes the enable/disable
//! decision back into `settings.json` — as a `+`/`-` pattern for the global
//! scope and as an inherit/load/unload override for the project scope.
//!
//! Deviations:
//!   * Class 2 (extension removal): the `extensions` resource type is gone, so
//!     the type list is `skills`, `prompts`, `themes`. The `extensions` key of
//!     an existing `packages` entry is still *read* where TypeScript checks
//!     whether a package still carries any filter — dropping that check would
//!     silently delete extension filters out of a settings.json that the
//!     TypeScript app still writes.
//!   * Class 1 (language idiom): `switchWriteScope` lives in `ResourceList`
//!     instead of the outer component. In TypeScript the list calls back into
//!     its owner; a Rust closure cannot re-enter the value that owns it, and
//!     the outer `writeScope` field has no other reader.
//!   * Class 1 (language idiom): the resolved-path types come from
//!     `core::resource_loader` (`ResolvedResource`, `ResolvedResources`) and
//!     `core::source_info` (`PathMetadata`), which are the same records
//!     `core/package-manager.ts` declares in TypeScript.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use notagent_tui::components::input::Input;
use notagent_tui::components::spacer::Spacer;
use notagent_tui::keybindings::keybindings_match;
use notagent_tui::keys::matches_key;
use notagent_tui::tui::{Component, ComponentRef, Container, Focusable, component_ref};
use notagent_tui::utils::{truncate_to_width_opts, visible_width};

use crate::config::CONFIG_DIR_NAME;
use crate::core::resource_loader::{ResolvedResource, ResolvedResources};
use crate::core::settings_manager::{
    PackageSource, PackageSourceFilter, Settings, SettingsManager,
};
use crate::core::source_info::{PathMetadata, SourceOrigin, SourceScope};
use crate::modes::interactive::theme::theme::{ThemeColor, theme};
use crate::utils::paths::{
    PathInputOptions, canonicalize_path, current_dir, is_absolute_path, is_local_path,
    node_relative, node_resolve, resolve_path,
};

use super::dynamic_border::DynamicBorder;
use super::keybinding_hints::{key_hint, raw_key_hint};

// ============================================================================
// Small Node helpers
// ============================================================================

/// `path.relative(from, to)`, including the resolution against the working
/// directory that Node performs for relative inputs.
fn relative(from: &str, to: &str) -> String {
    let cwd = current_dir();
    let resolve = |value: &str| {
        if is_absolute_path(value) {
            node_resolve(&[value])
        } else {
            node_resolve(&[&cwd, value])
        }
    };
    node_relative(&resolve(from), &resolve(to))
}

/// `path.basename(path)`.
fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `path.dirname(path)`.
fn dirname(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".to_string())
}

/// `path.join(left, right)`.
fn join(left: &str, right: &str) -> String {
    Path::new(left).join(right).to_string_lossy().into_owned()
}

/// `String.prototype.localeCompare(a, b)` for the ASCII package sources and
/// file names in play: case-insensitive, with lowercase winning a tie.
fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    let folded = a.to_lowercase().cmp(&b.to_lowercase());
    if folded != std::cmp::Ordering::Equal {
        return folded;
    }
    b.cmp(a)
}

// ============================================================================
// Types
// ============================================================================

/// The resource kinds the selector lists. `extensions` is gone with the
/// extension system (deviation class 2); the remaining order is the one
/// `RESOURCE_TYPES` fixes in TypeScript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceType {
    Skills,
    Prompts,
    Themes,
}

impl ResourceType {
    /// The settings key, which is also what the search matches against.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Skills => "skills",
            Self::Prompts => "prompts",
            Self::Themes => "themes",
        }
    }

    /// `RESOURCE_TYPE_LABELS`.
    fn label(self) -> &'static str {
        match self {
            Self::Skills => "Skills",
            Self::Prompts => "Prompts",
            Self::Themes => "Themes",
        }
    }
}

/// Which settings file the selector writes to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ConfigWriteScope {
    #[default]
    Global,
    Project,
}

/// `SettingsScope` of the TypeScript file: the two scopes a resource pattern
/// can be expressed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsScope {
    User,
    Project,
}

/// What the project settings say about an inherited resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectOverrideState {
    Inherit,
    Load,
    Unload,
}

/// `ScopedResolvedPaths`. The `ResolvedPaths` of TypeScript is
/// `ResolvedResources` here, minus its `extensions` list.
#[derive(Debug, Clone, Default)]
pub struct ScopedResolvedPaths {
    pub global: ResolvedResources,
    pub project: ResolvedResources,
}

/// One resource row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceItem {
    pub path: String,
    pub enabled: bool,
    pub metadata: PathMetadata,
    pub resource_type: ResourceType,
    pub display_name: String,
    pub group_key: String,
    pub subgroup_key: String,
}

/// The resources of one group that share a type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceSubgroup {
    pub resource_type: ResourceType,
    pub label: String,
    pub items: Vec<ResourceItem>,
}

/// Every resource that came from the same source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceGroup {
    pub key: String,
    pub label: String,
    pub scope: SourceScope,
    pub origin: SourceOrigin,
    pub source: String,
    pub subgroups: Vec<ResourceSubgroup>,
}

/// A row of the flattened list. TypeScript keeps object references here; the
/// port keeps the indices into `groups`, which identify the same rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlatEntry {
    Group {
        group: usize,
    },
    Subgroup {
        group: usize,
        subgroup: usize,
    },
    Item {
        group: usize,
        subgroup: usize,
        item: usize,
    },
}

// ============================================================================
// Group building
// ============================================================================

fn format_base_dir(base_dir: &str) -> String {
    let home_dir = dirs::home_dir()
        .map(|home| home.to_string_lossy().into_owned())
        .unwrap_or_default();

    let display_path = if !home_dir.is_empty() && base_dir == home_dir {
        "~".to_string()
    } else if !home_dir.is_empty() && base_dir.starts_with(&home_dir) {
        // Replace home prefix with ~, normalize separators for display
        format!("~{}", base_dir[home_dir.len()..].replace('\\', "/"))
    } else {
        base_dir.replace('\\', "/")
    };

    if display_path.ends_with('/') {
        display_path
    } else {
        format!("{display_path}/")
    }
}

/// The wire spelling of a scope, as the label interpolates it.
fn scope_str(scope: SourceScope) -> &'static str {
    match scope {
        SourceScope::User => "user",
        SourceScope::Project => "project",
        SourceScope::Temporary => "temporary",
    }
}

fn get_group_label(metadata: &PathMetadata, agent_dir: &str) -> String {
    if metadata.origin == SourceOrigin::Package {
        return format!("{} ({})", metadata.source, scope_str(metadata.scope));
    }
    // Top-level resources
    if metadata.source == "auto" {
        if let Some(base_dir) = metadata.base_dir.as_deref() {
            return if metadata.scope == SourceScope::User {
                format!("User ({})", format_base_dir(base_dir))
            } else {
                format!("Project ({})", format_base_dir(base_dir))
            };
        }
        return if metadata.scope == SourceScope::User {
            format!("User ({})", format_base_dir(agent_dir))
        } else {
            format!("Project ({CONFIG_DIR_NAME}/)")
        };
    }
    if metadata.scope == SourceScope::User {
        "User settings".to_string()
    } else {
        "Project settings".to_string()
    }
}

fn build_groups(resolved: &ResolvedResources, agent_dir: &str) -> Vec<ResourceGroup> {
    // A Vec keeps the insertion order a JS `Map` has.
    let mut groups: Vec<ResourceGroup> = Vec::new();

    let mut add_to_group = |resources: &[ResolvedResource], resource_type: ResourceType| {
        for res in resources {
            let metadata = &res.metadata;
            let group_key = format!(
                "{}:{}:{}:{}",
                match metadata.origin {
                    SourceOrigin::Package => "package",
                    SourceOrigin::TopLevel => "top-level",
                },
                scope_str(metadata.scope),
                metadata.source,
                metadata.base_dir.as_deref().unwrap_or("")
            );

            let group_index = match groups.iter().position(|group| group.key == group_key) {
                Some(index) => index,
                None => {
                    groups.push(ResourceGroup {
                        key: group_key.clone(),
                        label: get_group_label(metadata, agent_dir),
                        scope: metadata.scope,
                        origin: metadata.origin,
                        source: metadata.source.clone(),
                        subgroups: Vec::new(),
                    });
                    groups.len() - 1
                }
            };

            let group = &mut groups[group_index];
            let subgroup_key = format!("{group_key}:{}", resource_type.as_str());

            let subgroup_index = match group
                .subgroups
                .iter()
                .position(|subgroup| subgroup.resource_type == resource_type)
            {
                Some(index) => index,
                None => {
                    group.subgroups.push(ResourceSubgroup {
                        resource_type,
                        label: resource_type.label().to_string(),
                        items: Vec::new(),
                    });
                    group.subgroups.len() - 1
                }
            };

            let file_name = basename(&res.path);
            let parent_folder = basename(&dirname(&res.path));
            let display_name = if resource_type == ResourceType::Skills && file_name == "SKILL.md" {
                parent_folder
            } else {
                file_name
            };

            group.subgroups[subgroup_index].items.push(ResourceItem {
                path: res.path.clone(),
                enabled: res.enabled,
                metadata: metadata.clone(),
                resource_type,
                display_name,
                group_key: group_key.clone(),
                subgroup_key,
            });
        }
    };

    add_to_group(&resolved.skills, ResourceType::Skills);
    add_to_group(&resolved.prompts, ResourceType::Prompts);
    add_to_group(&resolved.themes, ResourceType::Themes);

    // Sort groups: packages first, then top-level; user before project
    groups.sort_by(|a, b| {
        if a.origin != b.origin {
            return if a.origin == SourceOrigin::Package {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        if a.scope != b.scope {
            return if a.scope == SourceScope::User {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        locale_compare(&a.source, &b.source)
    });

    // Sort subgroups within each group by type order, and items by name
    for group in &mut groups {
        group
            .subgroups
            .sort_by_key(|subgroup| subgroup.resource_type);
        for subgroup in &mut group.subgroups {
            subgroup
                .items
                .sort_by(|a, b| locale_compare(&a.display_name, &b.display_name));
        }
    }

    groups
}

// ============================================================================
// Header
// ============================================================================

/// `ConfigSelectorHeader`.
pub struct ConfigSelectorHeader {
    write_scope: ConfigWriteScope,
    project_mode_available: bool,
}

impl ConfigSelectorHeader {
    fn new(write_scope: ConfigWriteScope, project_mode_available: bool) -> Self {
        Self {
            write_scope,
            project_mode_available,
        }
    }

    fn set_write_scope(&mut self, write_scope: ConfigWriteScope) {
        self.write_scope = write_scope;
    }
}

impl Component for ConfigSelectorHeader {
    fn invalidate(&mut self) {}

    fn render(&mut self, width: usize) -> Vec<String> {
        let theme = theme();
        let title = theme.bold(if self.write_scope == ConfigWriteScope::Project {
            "Project Local Resources"
        } else {
            "Global Resources"
        });
        let sep = theme.fg(ThemeColor::Muted, " · ");
        let switch_hint = if self.project_mode_available {
            key_hint("tui.input.tab", "switch mode") + &sep
        } else {
            String::new()
        };
        let action_hint = if self.write_scope == ConfigWriteScope::Project {
            raw_key_hint("space", "cycle inherit/+/-")
        } else {
            raw_key_hint("space", "toggle")
        };
        let hint = format!(
            "{switch_hint}{action_hint}{sep}{}",
            raw_key_hint("esc", "close")
        );
        let spacing = (width as isize
            - visible_width(&title) as isize
            - visible_width(&hint) as isize)
            .max(1) as usize;
        let scope_hint = if self.write_scope == ConfigWriteScope::Project {
            theme.fg(
                ThemeColor::Muted,
                &format!("{CONFIG_DIR_NAME}/settings.json · inherited global resources are dimmed"),
            )
        } else {
            theme.fg(
                ThemeColor::Muted,
                &format!("~/{CONFIG_DIR_NAME}/agent/settings.json"),
            )
        };

        vec![
            truncate_to_width_opts(
                &format!("{title}{}{hint}", " ".repeat(spacing)),
                width,
                "",
                false,
            ),
            truncate_to_width_opts(&scope_hint, width, "", false),
        ]
    }
}

// ============================================================================
// Resource list
// ============================================================================

/// `ResourceList` — the scrolling list with the search input.
pub struct ResourceList {
    groups_global: Vec<ResourceGroup>,
    groups_project: Vec<ResourceGroup>,
    flat_items: Vec<FlatEntry>,
    filtered_items: Vec<FlatEntry>,
    selected_index: usize,
    search_input: Input,
    max_visible: usize,
    settings_manager: Arc<SettingsManager>,
    cwd: String,
    agent_dir: String,
    write_scope: ConfigWriteScope,
    inherited_enabled_by_key: std::collections::HashMap<String, bool>,
    header: Rc<RefCell<ConfigSelectorHeader>>,
    project_mode_available: bool,
    request_render: Rc<dyn Fn()>,
    on_cancel: Box<dyn FnMut()>,
    on_exit: Box<dyn FnMut()>,
    focused: bool,
}

impl ResourceList {
    #[allow(clippy::too_many_arguments)]
    fn new(
        groups_global: Vec<ResourceGroup>,
        groups_project: Vec<ResourceGroup>,
        settings_manager: Arc<SettingsManager>,
        cwd: String,
        agent_dir: String,
        terminal_height: Option<usize>,
        write_scope: ConfigWriteScope,
        header: Rc<RefCell<ConfigSelectorHeader>>,
        project_mode_available: bool,
        request_render: Rc<dyn Fn()>,
        on_cancel: Box<dyn FnMut()>,
        on_exit: Box<dyn FnMut()>,
    ) -> Self {
        let inherited_enabled_by_key = build_inherited_enabled_map(&groups_global);
        // 8 lines of chrome: top spacer + top border + spacer + header (2 lines)
        // + spacer + bottom spacer + bottom border
        let chrome = 8;
        let max_visible = (terminal_height.unwrap_or(24) as isize - chrome).max(5) as usize;

        let mut list = Self {
            groups_global,
            groups_project,
            flat_items: Vec::new(),
            filtered_items: Vec::new(),
            selected_index: 0,
            search_input: Input::new(),
            max_visible,
            settings_manager,
            cwd,
            agent_dir,
            write_scope,
            inherited_enabled_by_key,
            header,
            project_mode_available,
            request_render,
            on_cancel,
            on_exit,
            focused: false,
        };
        list.build_flat_list();
        list.filtered_items = list.flat_items.clone();
        list
    }

    /// `setWriteScope`.
    fn set_write_scope(&mut self, write_scope: ConfigWriteScope) {
        self.write_scope = write_scope;
        self.build_flat_list();
        let query = self.search_input.get_value().to_string();
        self.filter_items(&query);
    }

    /// The scope switch of the outer component: it flips the write scope of the
    /// list and of the shared header (deviation class 1, see the module docs).
    fn switch_write_scope(&mut self) {
        let next = if self.write_scope == ConfigWriteScope::Global {
            ConfigWriteScope::Project
        } else {
            ConfigWriteScope::Global
        };
        self.header.borrow_mut().set_write_scope(next);
        self.set_write_scope(next);
    }

    fn groups(&self) -> &Vec<ResourceGroup> {
        match self.write_scope {
            ConfigWriteScope::Global => &self.groups_global,
            ConfigWriteScope::Project => &self.groups_project,
        }
    }

    fn groups_mut(&mut self) -> &mut Vec<ResourceGroup> {
        match self.write_scope {
            ConfigWriteScope::Global => &mut self.groups_global,
            ConfigWriteScope::Project => &mut self.groups_project,
        }
    }

    fn item_at(&self, entry: FlatEntry) -> Option<&ResourceItem> {
        match entry {
            FlatEntry::Item {
                group,
                subgroup,
                item,
            } => self
                .groups()
                .get(group)
                .and_then(|group| group.subgroups.get(subgroup))
                .and_then(|subgroup| subgroup.items.get(item)),
            _ => None,
        }
    }

    fn build_flat_list(&mut self) {
        let mut flat_items = Vec::new();
        for (group_index, group) in self.groups().iter().enumerate() {
            flat_items.push(FlatEntry::Group { group: group_index });
            for (subgroup_index, subgroup) in group.subgroups.iter().enumerate() {
                flat_items.push(FlatEntry::Subgroup {
                    group: group_index,
                    subgroup: subgroup_index,
                });
                for item_index in 0..subgroup.items.len() {
                    flat_items.push(FlatEntry::Item {
                        group: group_index,
                        subgroup: subgroup_index,
                        item: item_index,
                    });
                }
            }
        }
        self.flat_items = flat_items;
        // Start selection on first item (not header)
        self.selected_index = self
            .flat_items
            .iter()
            .position(|entry| matches!(entry, FlatEntry::Item { .. }))
            .unwrap_or(0);
    }

    fn find_next_item(&self, from_index: usize, direction: isize) -> usize {
        let mut index = from_index as isize + direction;
        while index >= 0 && (index as usize) < self.filtered_items.len() {
            if matches!(self.filtered_items[index as usize], FlatEntry::Item { .. }) {
                return index as usize;
            }
            index += direction;
        }
        from_index // Stay at current if no item found
    }

    fn filter_items(&mut self, query: &str) {
        if query.trim().is_empty() {
            self.filtered_items = self.flat_items.clone();
            self.select_first_item();
            return;
        }

        let lower_query = query.to_lowercase();
        let mut matching_items: Vec<FlatEntry> = Vec::new();
        for entry in &self.flat_items {
            if let FlatEntry::Item { .. } = entry
                && let Some(item) = self.item_at(*entry)
                && (item.display_name.to_lowercase().contains(&lower_query)
                    || item.resource_type.as_str().contains(&lower_query)
                    || item.path.to_lowercase().contains(&lower_query))
            {
                matching_items.push(*entry);
            }
        }

        // Find which subgroups and groups contain matching items
        let mut matching_subgroups: Vec<(usize, usize)> = Vec::new();
        let mut matching_groups: Vec<usize> = Vec::new();
        for entry in &matching_items {
            if let FlatEntry::Item {
                group, subgroup, ..
            } = entry
            {
                if !matching_subgroups.contains(&(*group, *subgroup)) {
                    matching_subgroups.push((*group, *subgroup));
                }
                if !matching_groups.contains(group) {
                    matching_groups.push(*group);
                }
            }
        }

        self.filtered_items = self
            .flat_items
            .iter()
            .copied()
            .filter(|entry| match entry {
                FlatEntry::Group { group } => matching_groups.contains(group),
                FlatEntry::Subgroup { group, subgroup } => {
                    matching_subgroups.contains(&(*group, *subgroup))
                }
                FlatEntry::Item { .. } => matching_items.contains(entry),
            })
            .collect();

        self.select_first_item();
    }

    fn select_first_item(&mut self) {
        self.selected_index = self
            .filtered_items
            .iter()
            .position(|entry| matches!(entry, FlatEntry::Item { .. }))
            .unwrap_or(0);
    }

    /// `updateItem`: writes the new state onto the toggled row and onto the
    /// first row of the current scope that shares its path and type.
    fn update_item(&mut self, entry: FlatEntry, enabled: bool) {
        let Some(item) = self.item_at(entry).cloned() else {
            return;
        };
        if let FlatEntry::Item {
            group,
            subgroup,
            item: item_index,
        } = entry
        {
            self.groups_mut()[group].subgroups[subgroup].items[item_index].enabled = enabled;
        }
        for group in self.groups_mut() {
            for subgroup in &mut group.subgroups {
                if let Some(found) = subgroup.items.iter_mut().find(|other| {
                    other.path == item.path && other.resource_type == item.resource_type
                }) {
                    found.enabled = enabled;
                    return;
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Toggling
    // ------------------------------------------------------------------

    fn toggle_resource(&mut self, item: &ResourceItem) -> Option<bool> {
        if self.write_scope == ConfigWriteScope::Project {
            let state = self.get_next_override_state(item);
            if !self.set_project_resource_override(item, state) {
                return None;
            }
            return Some(match state {
                ProjectOverrideState::Inherit => self.get_inherited_enabled(item),
                ProjectOverrideState::Load => true,
                ProjectOverrideState::Unload => false,
            });
        }

        let enabled = !item.enabled;
        if item.metadata.origin == SourceOrigin::TopLevel {
            self.toggle_top_level_resource(item, enabled);
        } else {
            self.toggle_package_resource(item, enabled);
        }
        Some(enabled)
    }

    fn toggle_top_level_resource(&mut self, item: &ResourceItem, enabled: bool) {
        let scope = self.get_item_scope(item);
        let settings = match scope {
            SettingsScope::Project => self.settings_manager.get_project_settings(),
            SettingsScope::User => self.settings_manager.get_global_settings(),
        };

        let current = settings_paths(&settings, item.resource_type);

        // Generate pattern for this resource
        let pattern = self.get_resource_pattern(item);
        let disable_pattern = format!("-{pattern}");
        let enable_pattern = format!("+{pattern}");

        // Filter out existing patterns for this resource
        let mut updated: Vec<String> = current
            .into_iter()
            .filter(|entry| pattern_entry_target(entry) != pattern)
            .collect();

        updated.push(if enabled {
            enable_pattern
        } else {
            disable_pattern
        });

        match scope {
            SettingsScope::Project => {
                self.set_project_top_level_paths(item.resource_type, &updated)
            }
            SettingsScope::User => match item.resource_type {
                ResourceType::Skills => self.settings_manager.set_skill_paths(&updated),
                ResourceType::Prompts => self.settings_manager.set_prompt_template_paths(&updated),
                ResourceType::Themes => self.settings_manager.set_theme_paths(&updated),
            },
        }
    }

    fn toggle_package_resource(&mut self, item: &ResourceItem, enabled: bool) {
        let scope = self.get_item_scope(item);
        let settings = match scope {
            SettingsScope::Project => self.settings_manager.get_project_settings(),
            SettingsScope::User => self.settings_manager.get_global_settings(),
        };

        let mut packages = settings.packages.clone().unwrap_or_default();
        let Some(pkg_index) = packages
            .iter()
            .position(|pkg| package_source_name(pkg) == item.metadata.source)
        else {
            return;
        };

        // Convert string to object form if needed
        let mut pkg = to_filter(&packages[pkg_index]);

        // Get the resource array for this type
        let current = filter_paths(&pkg, item.resource_type).unwrap_or_default();

        // Generate pattern relative to package root
        let pattern = self.get_package_resource_pattern(item);
        let disable_pattern = format!("-{pattern}");
        let enable_pattern = format!("+{pattern}");

        // Filter out existing patterns for this resource
        let mut updated: Vec<String> = current
            .into_iter()
            .filter(|entry| pattern_entry_target(entry) != pattern)
            .collect();

        updated.push(if enabled {
            enable_pattern
        } else {
            disable_pattern
        });

        set_filter_paths(
            &mut pkg,
            item.resource_type,
            if updated.is_empty() {
                None
            } else {
                Some(updated)
            },
        );

        // Clean up empty filter object. The `extensions` key counts even though
        // the type is gone here, so an extension filter of the TypeScript app
        // survives (deviation class 2, see the module docs).
        let has_filters = pkg.extensions.is_some()
            || pkg.skills.is_some()
            || pkg.prompts.is_some()
            || pkg.themes.is_some();
        packages[pkg_index] = if has_filters {
            PackageSource::Filtered(pkg)
        } else {
            PackageSource::Source(pkg.source)
        };

        match scope {
            SettingsScope::Project => {
                let _ = self.settings_manager.set_project_packages(&packages);
            }
            SettingsScope::User => self.settings_manager.set_packages(&packages),
        }
    }

    // ------------------------------------------------------------------
    // Rendering helpers
    // ------------------------------------------------------------------

    fn render_checkbox(&self, item: &ResourceItem) -> String {
        let theme = theme();
        if self.write_scope == ConfigWriteScope::Project {
            return match self.get_project_override_state(item) {
                ProjectOverrideState::Load => theme.fg(ThemeColor::Success, "[+]"),
                ProjectOverrideState::Unload => theme.fg(ThemeColor::Warning, "[-]"),
                ProjectOverrideState::Inherit => {
                    theme.fg(ThemeColor::Dim, if item.enabled { "[x]" } else { "[ ]" })
                }
            };
        }
        if item.enabled {
            theme.fg(ThemeColor::Success, "[x]")
        } else {
            theme.fg(ThemeColor::Dim, "[ ]")
        }
    }

    fn get_item_suffix(&self, item: &ResourceItem) -> String {
        let theme = theme();
        if self.write_scope != ConfigWriteScope::Project {
            return String::new();
        }
        match self.get_project_override_state(item) {
            ProjectOverrideState::Load => theme.fg(ThemeColor::Muted, "  project load"),
            ProjectOverrideState::Unload => theme.fg(ThemeColor::Muted, "  project unload"),
            ProjectOverrideState::Inherit => {
                if self.is_inherited_global_item(item) {
                    theme.fg(ThemeColor::Dim, "  inherited global")
                } else {
                    String::new()
                }
            }
        }
    }

    fn is_dimmed_item(&self, item: &ResourceItem) -> bool {
        self.write_scope == ConfigWriteScope::Project
            && self.is_inherited_global_item(item)
            && self.get_project_override_state(item) == ProjectOverrideState::Inherit
    }

    // ------------------------------------------------------------------
    // Project overrides
    // ------------------------------------------------------------------

    fn set_project_resource_override(
        &mut self,
        item: &ResourceItem,
        state: ProjectOverrideState,
    ) -> bool {
        if item.metadata.origin == SourceOrigin::TopLevel {
            self.set_project_top_level_override(item, state)
        } else {
            self.set_project_package_override(item, state)
        }
    }

    fn set_project_top_level_override(
        &mut self,
        item: &ResourceItem,
        state: ProjectOverrideState,
    ) -> bool {
        let current = settings_paths(
            &self.settings_manager.get_project_settings(),
            item.resource_type,
        );
        let inherited = self.is_inherited_global_item(item);
        let pattern = if inherited {
            item.path.clone()
        } else {
            self.get_resource_pattern_for_scope(item, SettingsScope::Project)
        };
        let patterns = self.get_top_level_override_patterns(item, SettingsScope::Project);
        let mut updated: Vec<String> = current
            .into_iter()
            .filter(|entry| {
                let target = pattern_entry_target(entry);
                if is_prefixed(entry) && patterns.contains(&target.to_string()) {
                    return false;
                }
                !(state == ProjectOverrideState::Inherit && inherited && target == pattern)
            })
            .collect();
        if state != ProjectOverrideState::Inherit {
            if inherited && !updated.contains(&pattern) {
                updated.push(pattern.clone());
            }
            updated.push(format!(
                "{}{pattern}",
                if state == ProjectOverrideState::Load {
                    "+"
                } else {
                    "-"
                }
            ));
        }
        self.set_project_top_level_paths(item.resource_type, &updated);
        true
    }

    fn set_project_top_level_paths(&self, key: ResourceType, paths: &[String]) {
        // The project setters report a missing project settings file, which is
        // what TypeScript throws for; the selector offers the project scope only
        // when the caller says it exists.
        let _ = match key {
            ResourceType::Skills => self.settings_manager.set_project_skill_paths(paths),
            ResourceType::Prompts => self
                .settings_manager
                .set_project_prompt_template_paths(paths),
            ResourceType::Themes => self.settings_manager.set_project_theme_paths(paths),
        };
    }

    fn set_project_package_override(
        &mut self,
        item: &ResourceItem,
        state: ProjectOverrideState,
    ) -> bool {
        let mut packages = self
            .settings_manager
            .get_project_settings()
            .packages
            .clone()
            .unwrap_or_default();
        let mut pkg_index = packages.iter().position(|pkg| {
            self.package_source_string_matches(
                &item.metadata.source,
                self.get_item_scope(item),
                package_source_name(pkg),
                SettingsScope::Project,
            )
        });
        if pkg_index.is_none() {
            if state == ProjectOverrideState::Inherit {
                return false;
            }
            packages.push(self.create_package_override_source(item));
            pkg_index = Some(packages.len() - 1);
        }
        let pkg_index = pkg_index.expect("package index");
        let mut pkg = to_filter(&packages[pkg_index]);
        let pattern = self.get_package_resource_pattern(item);
        let mut updated: Vec<String> = filter_paths(&pkg, item.resource_type)
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| pattern_entry_target(entry) != pattern)
            .collect();
        if state != ProjectOverrideState::Inherit {
            updated.push(format!(
                "{}{pattern}",
                if state == ProjectOverrideState::Load {
                    "+"
                } else {
                    "-"
                }
            ));
        }
        set_filter_paths(
            &mut pkg,
            item.resource_type,
            if updated.is_empty() {
                None
            } else {
                Some(updated)
            },
        );
        // As above: `extensions` still counts as a filter.
        let has_filters = pkg.extensions.is_some()
            || pkg.skills.is_some()
            || pkg.prompts.is_some()
            || pkg.themes.is_some();
        if has_filters {
            packages[pkg_index] = PackageSource::Filtered(pkg);
        } else if pkg.autoload == Some(false) {
            packages.remove(pkg_index);
        } else {
            packages[pkg_index] = PackageSource::Source(pkg.source);
        }
        let _ = self.settings_manager.set_project_packages(&packages);
        true
    }

    fn get_next_override_state(&self, item: &ResourceItem) -> ProjectOverrideState {
        let state = self.get_project_override_state(item);
        let inherited_enabled = self.get_inherited_enabled(item);
        match state {
            ProjectOverrideState::Inherit => {
                if inherited_enabled {
                    ProjectOverrideState::Unload
                } else {
                    ProjectOverrideState::Load
                }
            }
            ProjectOverrideState::Unload => {
                if inherited_enabled {
                    ProjectOverrideState::Load
                } else {
                    ProjectOverrideState::Inherit
                }
            }
            ProjectOverrideState::Load => {
                if inherited_enabled {
                    ProjectOverrideState::Inherit
                } else {
                    ProjectOverrideState::Unload
                }
            }
        }
    }

    fn get_project_override_state(&self, item: &ResourceItem) -> ProjectOverrideState {
        if self.write_scope != ConfigWriteScope::Project {
            return ProjectOverrideState::Inherit;
        }
        if item.metadata.origin == SourceOrigin::TopLevel {
            return get_override_state_from_entries(
                &settings_paths(
                    &self.settings_manager.get_project_settings(),
                    item.resource_type,
                ),
                &self.get_top_level_override_patterns(item, SettingsScope::Project),
                false,
            );
        }
        let Some(PackageSource::Filtered(pkg)) =
            self.find_matching_package_source(item, SettingsScope::Project)
        else {
            return ProjectOverrideState::Inherit;
        };
        let Some(entries) = filter_paths(&pkg, item.resource_type) else {
            return ProjectOverrideState::Inherit;
        };
        get_override_state_from_entries(
            &entries,
            &[self.get_package_resource_pattern(item)],
            pkg.autoload != Some(false),
        )
    }

    fn get_inherited_enabled(&self, item: &ResourceItem) -> bool {
        self.inherited_enabled_by_key
            .get(&get_resource_item_key(item))
            .copied()
            .unwrap_or(match self.get_item_scope(item) {
                SettingsScope::User => item.enabled,
                SettingsScope::Project => true,
            })
    }

    fn is_inherited_global_item(&self, item: &ResourceItem) -> bool {
        self.get_item_scope(item) == SettingsScope::User
            || self
                .inherited_enabled_by_key
                .contains_key(&get_resource_item_key(item))
    }

    fn get_top_level_override_patterns(
        &self,
        item: &ResourceItem,
        scope: SettingsScope,
    ) -> Vec<String> {
        let base_dir = self.get_top_level_base_dir(scope);
        let mut patterns = vec![
            self.get_resource_pattern_for_scope(item, scope),
            item.path.clone(),
            relative(&base_dir, &item.path),
        ];
        if let Some(item_base_dir) = item.metadata.base_dir.as_deref() {
            patterns.push(relative(item_base_dir, &item.path));
        }
        patterns.dedup();
        patterns
    }

    fn get_resource_pattern_for_scope(&self, item: &ResourceItem, scope: SettingsScope) -> String {
        let source_scope = self.get_item_scope(item);
        if scope != source_scope {
            return item.path.clone();
        }
        let base_dir = item
            .metadata
            .base_dir
            .clone()
            .unwrap_or_else(|| self.get_top_level_base_dir(source_scope));
        relative(&base_dir, &item.path)
    }

    fn create_package_override_source(&self, item: &ResourceItem) -> PackageSource {
        let source = item.metadata.source.clone();
        if !is_local_path(&source) {
            return PackageSource::Filtered(PackageSourceFilter {
                source,
                autoload: Some(false),
                ..PackageSourceFilter::default()
            });
        }
        let source_path = trimmed_resolve(
            &source,
            &self.get_top_level_base_dir(self.get_item_scope(item)),
        );
        let relative_source = relative(
            &self.get_top_level_base_dir(SettingsScope::Project),
            &source_path,
        );
        PackageSource::Filtered(PackageSourceFilter {
            source: if relative_source.is_empty() {
                ".".to_string()
            } else {
                relative_source
            },
            autoload: Some(false),
            ..PackageSourceFilter::default()
        })
    }

    fn package_source_string_matches(
        &self,
        left_source: &str,
        left_scope: SettingsScope,
        right_source: &str,
        right_scope: SettingsScope,
    ) -> bool {
        if left_source == right_source {
            return true;
        }
        if !is_local_path(left_source) || !is_local_path(right_source) {
            return false;
        }
        let left = trimmed_resolve(left_source, &self.get_top_level_base_dir(left_scope));
        let right = trimmed_resolve(right_source, &self.get_top_level_base_dir(right_scope));
        left == right
    }

    fn find_matching_package_source(
        &self,
        item: &ResourceItem,
        target_scope: SettingsScope,
    ) -> Option<PackageSource> {
        let settings = match target_scope {
            SettingsScope::Project => self.settings_manager.get_project_settings(),
            SettingsScope::User => self.settings_manager.get_global_settings(),
        };
        settings
            .packages
            .clone()
            .unwrap_or_default()
            .into_iter()
            .find(|pkg| {
                self.package_source_string_matches(
                    &item.metadata.source,
                    self.get_item_scope(item),
                    package_source_name(pkg),
                    target_scope,
                )
            })
    }

    fn get_item_scope(&self, item: &ResourceItem) -> SettingsScope {
        if item.metadata.scope == SourceScope::Project {
            SettingsScope::Project
        } else {
            SettingsScope::User
        }
    }

    fn get_top_level_base_dir(&self, scope: SettingsScope) -> String {
        match scope {
            SettingsScope::Project => join(&self.cwd, CONFIG_DIR_NAME),
            SettingsScope::User => self.agent_dir.clone(),
        }
    }

    fn get_resource_pattern(&self, item: &ResourceItem) -> String {
        let scope = self.get_item_scope(item);
        let base_dir = item
            .metadata
            .base_dir
            .clone()
            .unwrap_or_else(|| self.get_top_level_base_dir(scope));
        relative(&base_dir, &item.path)
    }

    fn get_package_resource_pattern(&self, item: &ResourceItem) -> String {
        let base_dir = item
            .metadata
            .base_dir
            .clone()
            .unwrap_or_else(|| dirname(&item.path));
        relative(&base_dir, &item.path)
    }
}

/// `buildInheritedEnabledMap`.
fn build_inherited_enabled_map(
    groups: &[ResourceGroup],
) -> std::collections::HashMap<String, bool> {
    let mut result = std::collections::HashMap::new();
    for group in groups {
        for subgroup in &group.subgroups {
            for item in &subgroup.items {
                result.insert(get_resource_item_key(item), item.enabled);
            }
        }
    }
    result
}

fn get_resource_item_key(item: &ResourceItem) -> String {
    format!(
        "{}:{}",
        item.resource_type.as_str(),
        canonicalize_path(&item.path)
    )
}

fn is_prefixed(entry: &str) -> bool {
    entry.starts_with('!') || entry.starts_with('+') || entry.starts_with('-')
}

fn pattern_entry_target(entry: &str) -> &str {
    if is_prefixed(entry) {
        &entry[1..]
    } else {
        entry
    }
}

fn get_override_state_from_entries(
    entries: &[String],
    patterns: &[String],
    empty_array_is_unload: bool,
) -> ProjectOverrideState {
    if entries.is_empty() && empty_array_is_unload {
        return ProjectOverrideState::Unload;
    }
    let mut state = ProjectOverrideState::Inherit;
    for entry in entries {
        if !patterns
            .iter()
            .any(|pattern| pattern == pattern_entry_target(entry))
        {
            continue;
        }
        if entry.starts_with('!') || entry.starts_with('-') {
            state = ProjectOverrideState::Unload;
        } else {
            state = ProjectOverrideState::Load;
        }
    }
    state
}

/// `resolvePath(input, baseDir, { trim: true })`, falling back to the input the
/// way an unusable path would leave the TypeScript comparison unmatched.
fn trimmed_resolve(input: &str, base_dir: &str) -> String {
    resolve_path(
        input,
        base_dir,
        &PathInputOptions {
            trim: true,
            ..PathInputOptions::default()
        },
    )
    .unwrap_or_else(|_| input.to_string())
}

/// `settings[resourceType] ?? []`.
fn settings_paths(settings: &Settings, resource_type: ResourceType) -> Vec<String> {
    match resource_type {
        ResourceType::Skills => settings.skills.clone(),
        ResourceType::Prompts => settings.prompts.clone(),
        ResourceType::Themes => settings.themes.clone(),
    }
    .unwrap_or_default()
}

/// `typeof pkg === "string" ? pkg : pkg.source`.
fn package_source_name(pkg: &PackageSource) -> &str {
    match pkg {
        PackageSource::Source(source) => source,
        PackageSource::Filtered(filter) => &filter.source,
    }
}

/// `typeof pkg === "string" ? { source: pkg } : pkg`.
fn to_filter(pkg: &PackageSource) -> PackageSourceFilter {
    match pkg {
        PackageSource::Source(source) => PackageSourceFilter {
            source: source.clone(),
            ..PackageSourceFilter::default()
        },
        PackageSource::Filtered(filter) => filter.clone(),
    }
}

fn filter_paths(pkg: &PackageSourceFilter, resource_type: ResourceType) -> Option<Vec<String>> {
    match resource_type {
        ResourceType::Skills => pkg.skills.clone(),
        ResourceType::Prompts => pkg.prompts.clone(),
        ResourceType::Themes => pkg.themes.clone(),
    }
}

fn set_filter_paths(
    pkg: &mut PackageSourceFilter,
    resource_type: ResourceType,
    paths: Option<Vec<String>>,
) {
    match resource_type {
        ResourceType::Skills => pkg.skills = paths,
        ResourceType::Prompts => pkg.prompts = paths,
        ResourceType::Themes => pkg.themes = paths,
    }
}

impl Component for ResourceList {
    fn invalidate(&mut self) {}

    fn render(&mut self, width: usize) -> Vec<String> {
        let theme = theme();
        let mut lines: Vec<String> = Vec::new();

        // Search input
        lines.extend(self.search_input.render(width));
        lines.push(String::new());

        if self.filtered_items.is_empty() {
            lines.push(theme.fg(ThemeColor::Muted, "  No resources found"));
            return lines;
        }

        // Calculate visible range
        let start_index = (self.selected_index as isize - (self.max_visible / 2) as isize)
            .min(self.filtered_items.len() as isize - self.max_visible as isize)
            .max(0) as usize;
        let end_index = (start_index + self.max_visible).min(self.filtered_items.len());

        for index in start_index..end_index {
            let entry = self.filtered_items[index];
            let is_selected = index == self.selected_index;

            match entry {
                FlatEntry::Group { group } => {
                    // Main group header (no cursor)
                    let group = &self.groups()[group];
                    let inherited = self.write_scope == ConfigWriteScope::Project
                        && group.scope == SourceScope::User;
                    let label = theme.bold(&format!(
                        "{}{}",
                        group.label,
                        if inherited {
                            " · inherited global"
                        } else {
                            ""
                        }
                    ));
                    let group_line = theme.fg(
                        if inherited {
                            ThemeColor::Dim
                        } else {
                            ThemeColor::Accent
                        },
                        &label,
                    );
                    lines.push(truncate_to_width_opts(
                        &format!("  {group_line}"),
                        width,
                        "",
                        false,
                    ));
                }
                FlatEntry::Subgroup { group, subgroup } => {
                    // Subgroup header (indented, no cursor)
                    let group = &self.groups()[group];
                    let color = if self.write_scope == ConfigWriteScope::Project
                        && group.scope == SourceScope::User
                    {
                        ThemeColor::Dim
                    } else {
                        ThemeColor::Muted
                    };
                    let subgroup_line = theme.fg(color, &group.subgroups[subgroup].label);
                    lines.push(truncate_to_width_opts(
                        &format!("    {subgroup_line}"),
                        width,
                        "",
                        false,
                    ));
                }
                FlatEntry::Item { .. } => {
                    // Resource item (cursor only on items)
                    let Some(item) = self.item_at(entry).cloned() else {
                        continue;
                    };
                    let cursor = if is_selected { "> " } else { "  " };
                    let dimmed = self.is_dimmed_item(&item);
                    let name_text = if is_selected && !dimmed {
                        theme.bold(&item.display_name)
                    } else {
                        item.display_name.clone()
                    };
                    let name = if dimmed {
                        theme.fg(ThemeColor::Dim, &name_text)
                    } else {
                        name_text
                    };
                    lines.push(truncate_to_width_opts(
                        &format!(
                            "{cursor}    {} {name}{}",
                            self.render_checkbox(&item),
                            self.get_item_suffix(&item)
                        ),
                        width,
                        "...",
                        false,
                    ));
                }
            }
        }

        // Scroll indicator
        if start_index > 0 || end_index < self.filtered_items.len() {
            let item_count = self
                .filtered_items
                .iter()
                .filter(|entry| matches!(entry, FlatEntry::Item { .. }))
                .count();
            let current_item_index = self.filtered_items[..self.selected_index]
                .iter()
                .filter(|entry| matches!(entry, FlatEntry::Item { .. }))
                .count()
                + 1;
            lines.push(theme.fg(
                ThemeColor::Dim,
                &format!("  ({current_item_index}/{item_count})"),
            ));
        }

        lines
    }

    fn handle_input(&mut self, data: &str) {
        if keybindings_match(data, "tui.select.up") {
            self.selected_index = self.find_next_item(self.selected_index, -1);
            return;
        }
        if keybindings_match(data, "tui.select.down") {
            self.selected_index = self.find_next_item(self.selected_index, 1);
            return;
        }
        if keybindings_match(data, "tui.select.pageUp") {
            // Jump up by maxVisible, then find nearest item
            let mut target =
                (self.selected_index as isize - self.max_visible as isize).max(0) as usize;
            while target < self.filtered_items.len()
                && !matches!(self.filtered_items[target], FlatEntry::Item { .. })
            {
                target += 1;
            }
            if target < self.filtered_items.len() {
                self.selected_index = target;
            }
            return;
        }
        if keybindings_match(data, "tui.select.pageDown") {
            // Jump down by maxVisible, then find nearest item
            let mut target = (self.filtered_items.len() as isize - 1)
                .min(self.selected_index as isize + self.max_visible as isize);
            while target >= 0
                && !matches!(self.filtered_items[target as usize], FlatEntry::Item { .. })
            {
                target -= 1;
            }
            if target >= 0 {
                self.selected_index = target as usize;
            }
            return;
        }
        if keybindings_match(data, "tui.select.cancel") {
            (self.on_cancel)();
            return;
        }
        if matches_key(data, "ctrl+c") {
            (self.on_exit)();
            return;
        }
        if keybindings_match(data, "tui.input.tab") {
            if self.project_mode_available {
                self.switch_write_scope();
                (self.request_render)();
            }
            return;
        }
        if data == " " || keybindings_match(data, "tui.select.confirm") {
            let entry = self.filtered_items.get(self.selected_index).copied();
            if let Some(entry) = entry
                && let Some(item) = self.item_at(entry).cloned()
                && (self.write_scope == ConfigWriteScope::Project
                    || self.get_item_scope(&item) == SettingsScope::User)
                && let Some(new_enabled) = self.toggle_resource(&item)
            {
                self.update_item(entry, new_enabled);
                (self.request_render)();
            }
            return;
        }

        // Pass to search input
        self.search_input.handle_input(data);
        let query = self.search_input.get_value().to_string();
        self.filter_items(&query);
    }

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }
}

impl Focusable for ResourceList {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.search_input.set_focused(focused);
    }
}

// ============================================================================
// Component
// ============================================================================

/// `ConfigSelectorComponent`.
pub struct ConfigSelectorComponent {
    container: Container,
    resource_list: Rc<RefCell<ResourceList>>,
    focused: bool,
}

impl ConfigSelectorComponent {
    /// The TypeScript constructor, with `requestRender` as a shared callback
    /// and the terminal height still optional.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        resolved_paths: &ScopedResolvedPaths,
        settings_manager: Arc<SettingsManager>,
        cwd: impl Into<String>,
        agent_dir: impl Into<String>,
        on_close: Box<dyn FnMut()>,
        on_exit: Box<dyn FnMut()>,
        request_render: Rc<dyn Fn()>,
        terminal_height: Option<usize>,
        write_scope: ConfigWriteScope,
        project_mode_available: bool,
    ) -> Self {
        let cwd = cwd.into();
        let agent_dir = agent_dir.into();
        let mut container = Container::new();

        let groups_global = build_groups(&resolved_paths.global, &agent_dir);
        let groups_project = build_groups(&resolved_paths.project, &agent_dir);

        // Add header
        container.add_child(component_ref(Spacer::new(1)));
        container.add_child(component_ref(DynamicBorder::new(None)));
        container.add_child(component_ref(Spacer::new(1)));
        let header = Rc::new(RefCell::new(ConfigSelectorHeader::new(
            write_scope,
            project_mode_available,
        )));
        container.add_child(Rc::clone(&header) as ComponentRef);
        container.add_child(component_ref(Spacer::new(1)));

        // Resource list
        let resource_list = Rc::new(RefCell::new(ResourceList::new(
            groups_global,
            groups_project,
            settings_manager,
            cwd,
            agent_dir,
            terminal_height,
            write_scope,
            header,
            project_mode_available,
            request_render,
            on_close,
            on_exit,
        )));
        container.add_child(Rc::clone(&resource_list) as ComponentRef);

        // Bottom border
        container.add_child(component_ref(Spacer::new(1)));
        container.add_child(component_ref(DynamicBorder::new(None)));

        Self {
            container,
            resource_list,
            focused: false,
        }
    }

    /// `getResourceList()` — the focus target of `cli/config-selector.ts`.
    pub fn resource_list(&self) -> ComponentRef {
        Rc::clone(&self.resource_list) as ComponentRef
    }
}

impl Component for ConfigSelectorComponent {
    fn invalidate(&mut self) {
        self.container.invalidate();
    }

    fn render(&mut self, width: usize) -> Vec<String> {
        self.container.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.resource_list.borrow_mut().handle_input(data);
    }

    fn as_focusable(&mut self) -> Option<&mut dyn Focusable> {
        Some(self)
    }
}

impl Focusable for ConfigSelectorComponent {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.resource_list.borrow_mut().set_focused(focused);
    }
}
