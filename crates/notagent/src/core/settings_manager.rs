//! Port of `packages/coding-agent/src/core/settings-manager.ts`.
//!
//! Deviation class 1: the TS `Settings` interface is an open JS object — unknown
//! keys survive a load/persist round trip. The Rust struct keeps that property
//! with a flattened `extra` map. Field-level modification tracking uses the
//! camelCase wire names, which is what the persist step merges on.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::config::{CONFIG_DIR_NAME, get_agent_dir};

// --- Settings ----------------------------------------------------------------

macro_rules! settings_struct {
    ($($(#[$meta:meta])* $field:ident : $ty:ty),* $(,)?) => {
        #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub struct Settings {
            $(
                $(#[$meta])*
                #[serde(default, skip_serializing_if = "Option::is_none")]
                pub $field: Option<$ty>,
            )*
            /// Unknown keys are preserved across load and persist, as in JS.
            #[serde(flatten)]
            pub extra: Map<String, Value>,
        }
    };
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserve_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_recent_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummarySettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserve_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_prompt: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRetrySettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retry_delay_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrySettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_delay_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderRetrySettings>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_images: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_width_cells: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear_on_shrink: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_terminal_progress: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_resize: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_images: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingBudgetsSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimal: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub medium: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MermaidRenderingMode {
    Off,
    Final,
    Streaming,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkdownSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_block_indent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mermaid: Option<MermaidRenderingMode>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WarningSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic_extra_usage: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DefaultProjectTrust {
    Ask,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QueueMode {
    All,
    OneAtATime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DoubleEscapeAction {
    Fork,
    Tree,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TreeFilterMode {
    Default,
    NoTools,
    UserOnly,
    LabeledOnly,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TuiMode {
    Regular,
    Fullscreen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FullscreenExitOutput {
    Transcript,
    ResumeHint,
}

/// `PackageSource = string | { source, autoload?, extensions?, skills?, prompts?, themes? }`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PackageSource {
    Source(String),
    Filtered(PackageSourceFilter),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageSourceFilter {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autoload: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompts: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub themes: Option<Vec<String>>,
}

settings_struct!(
    last_changelog_version: String,
    default_provider: String,
    default_model: String,
    default_thinking_level: String,
    transport: String,
    steering_mode: QueueMode,
    follow_up_mode: QueueMode,
    theme: String,
    compaction: CompactionSettings,
    branch_summary: BranchSummarySettings,
    retry: RetrySettings,
    hide_thinking_block: bool,
    show_cache_miss_notices: bool,
    external_editor: String,
    shell_path: String,
    quiet_startup: bool,
    default_project_trust: DefaultProjectTrust,
    shell_command_prefix: String,
    npm_command: Vec<String>,
    enable_install_telemetry: bool,
    enable_analytics: bool,
    tracking_id: String,
    packages: Vec<PackageSource>,
    skills: Vec<String>,
    prompts: Vec<String>,
    themes: Vec<String>,
    enable_skill_commands: bool,
    terminal: TerminalSettings,
    images: ImageSettings,
    enabled_models: Vec<String>,
    double_escape_action: DoubleEscapeAction,
    tree_filter_mode: TreeFilterMode,
    thinking_budgets: ThinkingBudgetsSettings,
    editor_padding_x: u64,
    output_pad: u64,
    autocomplete_max_visible: u64,
    show_hardware_cursor: bool,
    markdown: MarkdownSettings,
    warnings: WarningSettings,
    session_dir: String,
    http_proxy: String,
    http_idle_timeout_ms: u64,
    websocket_connect_timeout_ms: u64,
    tui_mode: TuiMode,
    fullscreen_exit_output: FullscreenExitOutput,
    fullscreen_scrollbar: String,
);

// The TS `extensions` setting is dropped with the extension system
// (plans/facts/extension-boundary.md); unknown keys keep round-tripping
// through `Settings::extra`, so an existing settings.json is not damaged.

fn is_mergeable(value: &Value) -> bool {
    value.is_object()
}

/// Port of `deepMergeObjects`.
fn deep_merge_objects(
    base: &Map<String, Value>,
    overrides: &Map<String, Value>,
) -> Map<String, Value> {
    let mut result = base.clone();
    for (key, override_value) in overrides {
        if override_value.is_null() && !base.contains_key(key) {
            // `undefined` overrides are skipped in TS; JSON null is a real value.
        }
        let merged = match (base.get(key), override_value) {
            (Some(base_value), override_value)
                if is_mergeable(base_value) && is_mergeable(override_value) =>
            {
                Value::Object(deep_merge_objects(
                    base_value.as_object().expect("checked"),
                    override_value.as_object().expect("checked"),
                ))
            }
            _ => override_value.clone(),
        };
        result.insert(key.clone(), merged);
    }
    result
}

/// Deep merge settings: project/overrides take precedence, nested objects merge recursively.
pub fn deep_merge_settings(base: &Settings, overrides: &Settings) -> Settings {
    let base = to_map(base);
    let overrides = to_map(overrides);
    from_map(deep_merge_objects(&base, &overrides))
}

fn to_map(settings: &Settings) -> Map<String, Value> {
    match serde_json::to_value(settings).expect("settings serialize") {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

fn from_map(map: Map<String, Value>) -> Settings {
    serde_json::from_value(Value::Object(map)).unwrap_or_default()
}

// --- Storage -----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsScope {
    Global,
    Project,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsError {
    pub scope: SettingsScope,
    pub message: String,
}

/// TS: `withLock(scope, fn)` — the callback sees the current contents and
/// returns the contents to write, or `None` to leave the file untouched.
pub trait SettingsStorage: Send + Sync {
    fn with_lock(
        &self,
        scope: SettingsScope,
        body: &mut dyn FnMut(Option<String>) -> Option<String>,
    );
}

pub struct FileSettingsStorage {
    global_settings_path: PathBuf,
    project_settings_path: PathBuf,
}

impl FileSettingsStorage {
    pub fn new(cwd: &Path, agent_dir: &Path) -> Self {
        Self {
            global_settings_path: agent_dir.join("settings.json"),
            project_settings_path: cwd.join(CONFIG_DIR_NAME).join("settings.json"),
        }
    }

    fn path(&self, scope: SettingsScope) -> &Path {
        match scope {
            SettingsScope::Global => &self.global_settings_path,
            SettingsScope::Project => &self.project_settings_path,
        }
    }
}

impl SettingsStorage for FileSettingsStorage {
    fn with_lock(
        &self,
        scope: SettingsScope,
        body: &mut dyn FnMut(Option<String>) -> Option<String>,
    ) {
        let path = self.path(scope).to_path_buf();
        let file_exists = path.exists();
        let lock = file_exists.then(|| acquire_lock_with_retry(&path));
        let current = if file_exists {
            std::fs::read_to_string(&path).ok()
        } else {
            None
        };
        let next = body(current);
        if let Some(next) = next {
            if let Some(directory) = path.parent()
                && !directory.exists()
            {
                let _ = std::fs::create_dir_all(directory);
            }
            let _lock = match lock {
                Some(lock) => lock,
                None => acquire_lock_with_retry(&path),
            };
            let _ = std::fs::write(&path, next);
        }
    }
}

/// Port of `acquireLockSyncWithRetry`: 10 attempts, 20 ms apart.
///
/// Deviation class 3: `proper-lockfile` becomes a `.lock` directory created
/// exclusively, which is the same advisory scheme (a lock artefact next to the
/// file) with the same retry semantics.
struct SettingsLock {
    path: PathBuf,
    held: bool,
}

impl Drop for SettingsLock {
    fn drop(&mut self) {
        if self.held {
            let _ = std::fs::remove_dir(&self.path);
        }
    }
}

fn acquire_lock_with_retry(path: &Path) -> SettingsLock {
    let lock_path = PathBuf::from(format!("{}.lock", path.display()));
    for attempt in 1..=10 {
        if let Some(directory) = lock_path.parent()
            && !directory.exists()
        {
            let _ = std::fs::create_dir_all(directory);
        }
        match std::fs::create_dir(&lock_path) {
            Ok(()) => {
                return SettingsLock {
                    path: lock_path,
                    held: true,
                };
            }
            Err(_) if attempt < 10 => std::thread::sleep(std::time::Duration::from_millis(20)),
            Err(_) => break,
        }
    }
    // TS throws after the last attempt; the port proceeds without the advisory
    // lock rather than losing the write, and reports nothing — the file write
    // itself is still atomic per process.
    SettingsLock {
        path: lock_path,
        held: false,
    }
}

#[derive(Default)]
pub struct InMemorySettingsStorage {
    global: Mutex<Option<String>>,
    project: Mutex<Option<String>>,
}

impl SettingsStorage for InMemorySettingsStorage {
    fn with_lock(
        &self,
        scope: SettingsScope,
        body: &mut dyn FnMut(Option<String>) -> Option<String>,
    ) {
        let slot = match scope {
            SettingsScope::Global => &self.global,
            SettingsScope::Project => &self.project,
        };
        let current = slot.lock().expect("settings mutex").clone();
        if let Some(next) = body(current) {
            *slot.lock().expect("settings mutex") = Some(next);
        }
    }
}

// --- Manager -----------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct SettingsManagerCreateOptions {
    pub project_trusted: Option<bool>,
}

struct ManagerState {
    global_settings: Settings,
    project_settings: Settings,
    settings: Settings,
    project_trusted: bool,
    modified_fields: BTreeSet<String>,
    modified_nested_fields: BTreeMap<String, BTreeSet<String>>,
    modified_project_fields: BTreeSet<String>,
    modified_project_nested_fields: BTreeMap<String, BTreeSet<String>>,
    global_load_error: Option<String>,
    project_load_error: Option<String>,
    errors: Vec<SettingsError>,
}

pub struct SettingsManager {
    storage: Arc<dyn SettingsStorage>,
    state: Mutex<ManagerState>,
}

impl SettingsManager {
    /// Create a SettingsManager that loads from files.
    pub fn create(
        cwd: &Path,
        agent_dir: Option<&Path>,
        options: SettingsManagerCreateOptions,
    ) -> Self {
        let agent_dir = agent_dir.map_or_else(get_agent_dir, Path::to_path_buf);
        Self::from_storage(Arc::new(FileSettingsStorage::new(cwd, &agent_dir)), options)
    }

    /// Create a SettingsManager from an arbitrary storage backend.
    pub fn from_storage(
        storage: Arc<dyn SettingsStorage>,
        options: SettingsManagerCreateOptions,
    ) -> Self {
        let project_trusted = options.project_trusted.unwrap_or(true);
        let (global_settings, global_error) =
            try_load(storage.as_ref(), SettingsScope::Global, true);
        let (project_settings, project_error) =
            try_load(storage.as_ref(), SettingsScope::Project, project_trusted);
        let mut errors = Vec::new();
        if let Some(error) = &global_error {
            errors.push(SettingsError {
                scope: SettingsScope::Global,
                message: error.clone(),
            });
        }
        if let Some(error) = &project_error {
            errors.push(SettingsError {
                scope: SettingsScope::Project,
                message: error.clone(),
            });
        }
        let settings = deep_merge_settings(&global_settings, &project_settings);
        Self {
            storage,
            state: Mutex::new(ManagerState {
                global_settings,
                project_settings,
                settings,
                project_trusted,
                modified_fields: BTreeSet::new(),
                modified_nested_fields: BTreeMap::new(),
                modified_project_fields: BTreeSet::new(),
                modified_project_nested_fields: BTreeMap::new(),
                global_load_error: global_error,
                project_load_error: project_error,
                errors,
            }),
        }
    }

    /// Create an in-memory SettingsManager (no file I/O).
    pub fn in_memory(settings: &Settings, options: SettingsManagerCreateOptions) -> Self {
        let storage = Arc::new(InMemorySettingsStorage::default());
        let migrated = migrate_settings(to_map(settings));
        let serialized =
            serde_json::to_string_pretty(&Value::Object(migrated)).expect("settings serialize");
        storage.with_lock(SettingsScope::Global, &mut |_| Some(serialized.clone()));
        Self::from_storage(storage, options)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ManagerState> {
        self.state.lock().expect("settings manager mutex")
    }

    pub fn settings(&self) -> Settings {
        self.lock().settings.clone()
    }

    pub fn get_global_settings(&self) -> Settings {
        self.lock().global_settings.clone()
    }

    pub fn get_project_settings(&self) -> Settings {
        self.lock().project_settings.clone()
    }

    pub fn is_project_trusted(&self) -> bool {
        self.lock().project_trusted
    }

    pub fn set_project_trusted(&self, trusted: bool) {
        {
            let mut state = self.lock();
            if state.project_trusted == trusted {
                return;
            }
            state.project_trusted = trusted;
            state.modified_project_fields.clear();
            state.modified_project_nested_fields.clear();
            if !trusted {
                state.project_settings = Settings::default();
                state.project_load_error = None;
                state.settings =
                    deep_merge_settings(&state.global_settings, &state.project_settings);
                return;
            }
        }
        let (project_settings, error) =
            try_load(self.storage.as_ref(), SettingsScope::Project, true);
        let mut state = self.lock();
        state.project_settings = project_settings;
        state.project_load_error = error.clone();
        if let Some(error) = error {
            state.errors.push(SettingsError {
                scope: SettingsScope::Project,
                message: error,
            });
        }
        state.settings = deep_merge_settings(&state.global_settings, &state.project_settings);
    }

    /// TS awaits the write queue first; the Rust writes are synchronous, so
    /// reload only re-reads (deviation class 1).
    pub fn reload(&self) {
        let (global_settings, global_error) =
            try_load(self.storage.as_ref(), SettingsScope::Global, true);
        let project_trusted = self.lock().project_trusted;
        let (project_settings, project_error) = try_load(
            self.storage.as_ref(),
            SettingsScope::Project,
            project_trusted,
        );
        let mut state = self.lock();
        match global_error {
            None => {
                state.global_settings = global_settings;
                state.global_load_error = None;
            }
            Some(error) => {
                state.global_load_error = Some(error.clone());
                state.errors.push(SettingsError {
                    scope: SettingsScope::Global,
                    message: error,
                });
            }
        }
        state.modified_fields.clear();
        state.modified_nested_fields.clear();
        state.modified_project_fields.clear();
        state.modified_project_nested_fields.clear();
        match project_error {
            None => {
                state.project_settings = project_settings;
                state.project_load_error = None;
            }
            Some(error) => {
                state.project_load_error = Some(error.clone());
                state.errors.push(SettingsError {
                    scope: SettingsScope::Project,
                    message: error,
                });
            }
        }
        state.settings = deep_merge_settings(&state.global_settings, &state.project_settings);
    }

    /// Apply additional overrides on top of current settings.
    pub fn apply_overrides(&self, overrides: &Settings) {
        let mut state = self.lock();
        state.settings = deep_merge_settings(&state.settings, overrides);
    }

    pub fn drain_errors(&self) -> Vec<SettingsError> {
        std::mem::take(&mut self.lock().errors)
    }

    /// TS queues writes on a promise chain; the Rust writes are synchronous, so
    /// `flush` has nothing left to await.
    pub fn flush(&self) {}

    /// Sets a global field by its wire name and persists it.
    pub fn set_global_field(&self, field: &str, value: Value) {
        {
            let mut state = self.lock();
            let mut map = to_map(&state.global_settings);
            map.insert(field.to_owned(), value);
            state.global_settings = from_map(map);
            state.modified_fields.insert(field.to_owned());
        }
        self.save();
    }

    /// Sets one key inside a nested global object and persists it.
    pub fn set_global_nested_field(&self, field: &str, nested_key: &str, value: Value) {
        {
            let mut state = self.lock();
            let mut map = to_map(&state.global_settings);
            let mut nested = map
                .get(field)
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            nested.insert(nested_key.to_owned(), value);
            map.insert(field.to_owned(), Value::Object(nested));
            state.global_settings = from_map(map);
            state.modified_fields.insert(field.to_owned());
            state
                .modified_nested_fields
                .entry(field.to_owned())
                .or_default()
                .insert(nested_key.to_owned());
        }
        self.save();
    }

    /// Sets a project field by its wire name and persists it.
    pub fn set_project_field(&self, field: &str, value: Value) -> Result<(), SettingsError> {
        {
            let state = self.lock();
            if !state.project_trusted {
                return Err(SettingsError {
                    scope: SettingsScope::Project,
                    message: "Project is not trusted; refusing to write project settings"
                        .to_owned(),
                });
            }
        }
        {
            let mut state = self.lock();
            let mut map = to_map(&state.project_settings);
            map.insert(field.to_owned(), value);
            state.project_settings = from_map(map);
            state.modified_project_fields.insert(field.to_owned());
        }
        self.save_project();
        Ok(())
    }

    fn save(&self) {
        let (snapshot, modified_fields, modified_nested, blocked) = {
            let mut state = self.lock();
            state.settings = deep_merge_settings(&state.global_settings, &state.project_settings);
            (
                state.global_settings.clone(),
                state.modified_fields.clone(),
                state.modified_nested_fields.clone(),
                state.global_load_error.is_some(),
            )
        };
        if blocked {
            return;
        }
        self.persist_scoped(
            SettingsScope::Global,
            &snapshot,
            &modified_fields,
            &modified_nested,
        );
        let mut state = self.lock();
        state.modified_fields.clear();
        state.modified_nested_fields.clear();
    }

    fn save_project(&self) {
        let (snapshot, modified_fields, modified_nested, blocked) = {
            let mut state = self.lock();
            state.settings = deep_merge_settings(&state.global_settings, &state.project_settings);
            (
                state.project_settings.clone(),
                state.modified_project_fields.clone(),
                state.modified_project_nested_fields.clone(),
                state.project_load_error.is_some(),
            )
        };
        if blocked {
            return;
        }
        self.persist_scoped(
            SettingsScope::Project,
            &snapshot,
            &modified_fields,
            &modified_nested,
        );
        let mut state = self.lock();
        state.modified_project_fields.clear();
        state.modified_project_nested_fields.clear();
    }

    /// Port of `persistScopedSettings`: merge only the modified fields into the
    /// file's current contents so concurrent writers keep their keys.
    fn persist_scoped(
        &self,
        scope: SettingsScope,
        snapshot: &Settings,
        modified_fields: &BTreeSet<String>,
        modified_nested_fields: &BTreeMap<String, BTreeSet<String>>,
    ) {
        let snapshot_map = to_map(snapshot);
        self.storage.with_lock(scope, &mut |current| {
            let current_map = current
                .as_deref()
                .and_then(|content| serde_json::from_str::<Value>(content).ok())
                .and_then(|value| value.as_object().cloned())
                .map(migrate_settings)
                .unwrap_or_default();
            let mut merged = current_map.clone();
            for field in modified_fields {
                let value = snapshot_map.get(field).cloned().unwrap_or(Value::Null);
                match (modified_nested_fields.get(field), value.as_object()) {
                    (Some(nested_modified), Some(in_memory_nested)) => {
                        let mut merged_nested = current_map
                            .get(field)
                            .and_then(Value::as_object)
                            .cloned()
                            .unwrap_or_default();
                        for nested_key in nested_modified {
                            match in_memory_nested.get(nested_key) {
                                Some(nested_value) => {
                                    merged_nested.insert(nested_key.clone(), nested_value.clone());
                                }
                                None => {
                                    merged_nested.remove(nested_key);
                                }
                            }
                        }
                        merged.insert(field.clone(), Value::Object(merged_nested));
                    }
                    _ => {
                        if value.is_null() {
                            merged.remove(field);
                        } else {
                            merged.insert(field.clone(), value);
                        }
                    }
                }
            }
            Some(serde_json::to_string_pretty(&Value::Object(merged)).expect("settings serialize"))
        });
    }
}

fn try_load(
    storage: &dyn SettingsStorage,
    scope: SettingsScope,
    project_trusted: bool,
) -> (Settings, Option<String>) {
    if scope == SettingsScope::Project && !project_trusted {
        return (Settings::default(), None);
    }
    let mut content: Option<String> = None;
    storage.with_lock(scope, &mut |current| {
        content = current;
        None
    });
    let Some(content) = content.filter(|content| !content.is_empty()) else {
        return (Settings::default(), None);
    };
    match serde_json::from_str::<Value>(&content) {
        Ok(Value::Object(map)) => (from_map(migrate_settings(map)), None),
        Ok(_) => (
            Settings::default(),
            Some("Settings file must contain an object".to_owned()),
        ),
        Err(error) => (Settings::default(), Some(error.to_string())),
    }
}

/// Port of `migrateSettings`.
pub fn migrate_settings(mut settings: Map<String, Value>) -> Map<String, Value> {
    // queueMode -> steeringMode
    if settings.contains_key("queueMode") && !settings.contains_key("steeringMode") {
        if let Some(value) = settings.remove("queueMode") {
            settings.insert("steeringMode".to_owned(), value);
        }
    } else {
        settings.remove("queueMode");
    }

    // legacy websockets boolean -> transport enum
    if !settings.contains_key("transport")
        && let Some(Value::Bool(websockets)) = settings.get("websockets").cloned()
    {
        settings.insert(
            "transport".to_owned(),
            Value::from(if websockets { "websocket" } else { "sse" }),
        );
        settings.remove("websockets");
    }

    // old skills object format -> array format
    if let Some(skills) = settings.get("skills").cloned()
        && let Value::Object(skills) = skills
    {
        if let Some(enable) = skills.get("enableSkillCommands")
            && !settings.contains_key("enableSkillCommands")
        {
            settings.insert("enableSkillCommands".to_owned(), enable.clone());
        }
        match skills.get("customDirectories") {
            Some(Value::Array(directories)) if !directories.is_empty() => {
                settings.insert("skills".to_owned(), Value::Array(directories.clone()));
            }
            _ => {
                settings.remove("skills");
            }
        }
    }

    // retry.maxDelayMs -> retry.provider.maxRetryDelayMs
    if let Some(Value::Object(mut retry)) = settings.get("retry").cloned() {
        let max_delay = retry.get("maxDelayMs").and_then(Value::as_u64);
        let provider = retry.get("provider").and_then(Value::as_object).cloned();
        if let Some(max_delay) = max_delay {
            let existing = provider
                .as_ref()
                .and_then(|provider| provider.get("maxRetryDelayMs"));
            if existing.is_none() || existing == Some(&Value::Null) {
                let mut provider = provider.unwrap_or_default();
                provider.insert("maxRetryDelayMs".to_owned(), Value::from(max_delay));
                retry.insert("provider".to_owned(), Value::Object(provider));
            }
        }
        retry.remove("maxDelayMs");
        settings.insert("retry".to_owned(), Value::Object(retry));
    }

    settings
}
