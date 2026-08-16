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
    pub image_width_cells: Option<Value>,
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
    pub mermaid: Option<Value>,
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

// The fields typed as `Value` are the ones the TS accessors re-check at read time
// (`typeof value === "string"`, `=== "fullscreen"`, `parseTimeoutSetting`, …). Keeping
// them untyped means a settings.json that the TS app tolerates still loads here, and
// the fallback lives in the accessor, exactly as in TS.
settings_struct!(
    last_changelog_version: String,
    default_provider: String,
    default_model: String,
    /// Addition over the TS original (user decision 2026-08-16, v0.1.6):
    /// the model delegated subagents run on; unset means inherit.
    subagent_provider: String,
    subagent_model: String,
    /// Port addition (v0.1.7): gate of the `find_codebase` tool; absent = on.
    find_codebase_enabled: bool,
    /// Port addition (v0.1.9): chat-block style, "standard" | "badge";
    /// absent = badge (user decision 2026-08-17).
    block_style: String,
    default_thinking_level: Value,
    transport: Value,
    steering_mode: Value,
    follow_up_mode: Value,
    theme: Value,
    compaction: CompactionSettings,
    branch_summary: BranchSummarySettings,
    retry: RetrySettings,
    hide_thinking_block: bool,
    show_cache_miss_notices: bool,
    external_editor: String,
    shell_path: String,
    quiet_startup: bool,
    default_project_trust: Value,
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
    double_escape_action: Value,
    tree_filter_mode: Value,
    thinking_budgets: ThinkingBudgetsSettings,
    editor_padding_x: Value,
    output_pad: Value,
    autocomplete_max_visible: Value,
    show_hardware_cursor: bool,
    markdown: MarkdownSettings,
    warnings: WarningSettings,
    session_dir: String,
    http_proxy: String,
    http_idle_timeout_ms: Value,
    websocket_connect_timeout_ms: Value,
    tui_mode: Value,
    fullscreen_exit_output: Value,
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

    /// Sets several global fields and persists them in one write, the way TS marks
    /// two fields modified before a single `save()`.
    pub fn set_global_fields(&self, fields: &[(&str, Value)]) {
        {
            let mut state = self.lock();
            let mut map = to_map(&state.global_settings);
            for (field, value) in fields {
                map.insert((*field).to_owned(), value.clone());
                state.modified_fields.insert((*field).to_owned());
            }
            state.global_settings = from_map(map);
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

// =============================================================================
// Typed accessors (settings-manager.ts:666-1272)
// =============================================================================

/// `ScrollViewScrollbar` — the visibility of the fullscreen scrollbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScrollViewScrollbar {
    Auto,
    Always,
    Hidden,
}

/// `{ enabled, reserveTokens, keepRecentTokens }`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedCompactionSettings {
    pub enabled: bool,
    pub reserve_tokens: u64,
    pub keep_recent_tokens: u64,
}

/// `{ reserveTokens, skipPrompt }`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedBranchSummarySettings {
    pub reserve_tokens: u64,
    pub skip_prompt: bool,
}

/// `{ enabled, maxRetries, baseDelayMs }`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedRetrySettings {
    pub enabled: bool,
    pub max_retries: u64,
    pub base_delay_ms: u64,
}

/// `{ timeoutMs?, maxRetries?, maxRetryDelayMs }`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedProviderRetrySettings {
    pub timeout_ms: Option<u64>,
    pub max_retries: Option<u64>,
    pub max_retry_delay_ms: u64,
}

pub const DEFAULT_HTTP_IDLE_TIMEOUT_MS: u64 = 300_000;

/// Port of `parseHttpIdleTimeoutMs` (`core/http-dispatcher.ts`).
pub fn parse_http_idle_timeout_ms(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.eq_ignore_ascii_case("disabled") {
                return Some(0);
            }
            if trimmed.is_empty() {
                return None;
            }
            let number = trimmed.parse::<f64>().ok()?;
            parse_http_idle_timeout_ms(Some(&Value::from(number)))
        }
        Value::Number(number) => {
            let number = number.as_f64()?;
            if !number.is_finite() || number < 0.0 {
                return None;
            }
            Some(number.floor() as u64)
        }
        _ => None,
    }
}

/// Port of `parseTimeoutSetting`.
fn parse_timeout_setting(value: Option<&Value>, setting_name: &str) -> Result<Option<u64>, String> {
    if let Some(timeout_ms) = parse_http_idle_timeout_ms(value) {
        return Ok(Some(timeout_ms));
    }
    match present(value) {
        Some(value) => Err(format!(
            "Invalid {setting_name} setting: {}",
            display_json(value)
        )),
        None => Ok(None),
    }
}

/// JSON `null` is JS `undefined` here: both mean "not configured".
fn present(value: Option<&Value>) -> Option<&Value> {
    value.filter(|value| !value.is_null())
}

/// `String(value)` for error messages.
fn display_json(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn as_str(value: &Option<Value>) -> Option<&str> {
    value.as_ref().and_then(Value::as_str)
}

fn as_u64_or(value: &Option<Value>, default: u64) -> u64 {
    present(value.as_ref())
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite())
        .map_or(default, |number| number.max(0.0).floor() as u64)
}

impl SettingsManager {
    fn settings_snapshot(&self) -> Settings {
        self.lock().settings.clone()
    }

    fn global_snapshot(&self) -> Settings {
        self.lock().global_settings.clone()
    }

    pub fn get_last_changelog_version(&self) -> Option<String> {
        self.settings_snapshot().last_changelog_version
    }

    pub fn set_last_changelog_version(&self, version: &str) {
        self.set_global_field("lastChangelogVersion", Value::from(version));
    }

    pub fn get_session_dir(&self) -> Option<String> {
        let session_dir = self.settings_snapshot().session_dir?;
        Some(normalize_setting_path(&session_dir))
    }

    pub fn get_default_provider(&self) -> Option<String> {
        self.settings_snapshot().default_provider
    }

    pub fn get_default_model(&self) -> Option<String> {
        self.settings_snapshot().default_model
    }

    pub fn set_default_provider(&self, provider: &str) {
        self.set_global_field("defaultProvider", Value::from(provider));
    }

    pub fn set_default_model(&self, model_id: &str) {
        self.set_global_field("defaultModel", Value::from(model_id));
    }

    pub fn set_default_model_and_provider(&self, provider: &str, model_id: &str) {
        self.set_global_fields(&[
            ("defaultProvider", Value::from(provider)),
            ("defaultModel", Value::from(model_id)),
        ]);
    }

    /// Chat-block style (port addition, v0.1.9): `"standard"` is the filled
    /// surface, anything else — including absent — is the badge style, the
    /// default by user decision.
    pub fn get_block_style_badge(&self) -> bool {
        self.settings_snapshot()
            .block_style
            .is_none_or(|style| style != "standard")
    }

    pub fn set_block_style_setting(&self, badge: bool) {
        self.set_global_field(
            "blockStyle",
            Value::from(if badge { "badge" } else { "standard" }),
        );
    }

    /// The `find_codebase` gate (port addition, v0.1.7). Absent means
    /// enabled, matching the reference default for `cb_search_enabled`.
    pub fn get_find_codebase_enabled(&self) -> bool {
        self.settings_snapshot()
            .find_codebase_enabled
            .unwrap_or(true)
    }

    pub fn set_find_codebase_enabled(&self, enabled: bool) {
        self.set_global_field("findCodebaseEnabled", Value::from(enabled));
    }

    pub fn get_subagent_provider(&self) -> Option<String> {
        self.settings_snapshot().subagent_provider
    }

    pub fn get_subagent_model(&self) -> Option<String> {
        self.settings_snapshot().subagent_model
    }

    pub fn set_subagent_model_and_provider(&self, provider: &str, model_id: &str) {
        self.set_global_fields(&[
            ("subagentProvider", Value::from(provider)),
            ("subagentModel", Value::from(model_id)),
        ]);
    }

    /// Back to inheriting the main model. `Null` lands as `None` in the typed
    /// field and `skip_serializing_if` drops the key from settings.json.
    pub fn clear_subagent_model(&self) {
        self.set_global_fields(&[
            ("subagentProvider", Value::Null),
            ("subagentModel", Value::Null),
        ]);
    }

    pub fn get_steering_mode(&self) -> QueueMode {
        queue_mode(&self.settings_snapshot().steering_mode)
    }

    pub fn set_steering_mode(&self, mode: QueueMode) {
        self.set_global_field("steeringMode", json_of(&mode));
    }

    pub fn get_follow_up_mode(&self) -> QueueMode {
        queue_mode(&self.settings_snapshot().follow_up_mode)
    }

    pub fn set_follow_up_mode(&self, mode: QueueMode) {
        self.set_global_field("followUpMode", json_of(&mode));
    }

    /// The configured theme, including package-qualified names.
    pub fn get_theme_setting(&self) -> Option<String> {
        as_str(&self.settings_snapshot().theme).map(str::to_owned)
    }

    /// The theme of a builtin or user theme file; package themes resolve later.
    pub fn get_theme(&self) -> Option<String> {
        self.get_theme_setting()
            .filter(|theme| !theme.contains('/'))
    }

    pub fn set_theme(&self, theme: &str) {
        self.set_global_field("theme", Value::from(theme));
    }

    pub fn get_default_thinking_level(&self) -> Option<String> {
        as_str(&self.settings_snapshot().default_thinking_level).map(str::to_owned)
    }

    pub fn set_default_thinking_level(&self, level: &str) {
        self.set_global_field("defaultThinkingLevel", Value::from(level));
    }

    pub fn get_transport(&self) -> String {
        as_str(&self.settings_snapshot().transport)
            .unwrap_or("auto")
            .to_owned()
    }

    pub fn set_transport(&self, transport: &str) {
        self.set_global_field("transport", Value::from(transport));
    }

    pub fn get_compaction_enabled(&self) -> bool {
        self.settings_snapshot()
            .compaction
            .and_then(|compaction| compaction.enabled)
            .unwrap_or(true)
    }

    pub fn set_compaction_enabled(&self, enabled: bool) {
        self.set_global_nested_field("compaction", "enabled", Value::from(enabled));
    }

    pub fn get_compaction_reserve_tokens(&self) -> u64 {
        self.settings_snapshot()
            .compaction
            .and_then(|compaction| compaction.reserve_tokens)
            .unwrap_or(16_384)
    }

    pub fn get_compaction_keep_recent_tokens(&self) -> u64 {
        self.settings_snapshot()
            .compaction
            .and_then(|compaction| compaction.keep_recent_tokens)
            .unwrap_or(20_000)
    }

    pub fn get_compaction_settings(&self) -> ResolvedCompactionSettings {
        ResolvedCompactionSettings {
            enabled: self.get_compaction_enabled(),
            reserve_tokens: self.get_compaction_reserve_tokens(),
            keep_recent_tokens: self.get_compaction_keep_recent_tokens(),
        }
    }

    pub fn get_branch_summary_settings(&self) -> ResolvedBranchSummarySettings {
        let branch_summary = self.settings_snapshot().branch_summary;
        ResolvedBranchSummarySettings {
            reserve_tokens: branch_summary
                .as_ref()
                .and_then(|settings| settings.reserve_tokens)
                .unwrap_or(16_384),
            skip_prompt: branch_summary
                .and_then(|settings| settings.skip_prompt)
                .unwrap_or(false),
        }
    }

    pub fn get_branch_summary_skip_prompt(&self) -> bool {
        self.settings_snapshot()
            .branch_summary
            .and_then(|settings| settings.skip_prompt)
            .unwrap_or(false)
    }

    pub fn get_retry_enabled(&self) -> bool {
        self.settings_snapshot()
            .retry
            .and_then(|retry| retry.enabled)
            .unwrap_or(true)
    }

    pub fn set_retry_enabled(&self, enabled: bool) {
        self.set_global_nested_field("retry", "enabled", Value::from(enabled));
    }

    pub fn get_retry_settings(&self) -> ResolvedRetrySettings {
        let retry = self.settings_snapshot().retry;
        ResolvedRetrySettings {
            enabled: self.get_retry_enabled(),
            max_retries: retry
                .as_ref()
                .and_then(|retry| retry.max_retries)
                .unwrap_or(3),
            base_delay_ms: retry.and_then(|retry| retry.base_delay_ms).unwrap_or(2_000),
        }
    }

    pub fn get_http_idle_timeout_ms(&self) -> Result<u64, String> {
        let settings = self.settings_snapshot();
        Ok(
            parse_timeout_setting(settings.http_idle_timeout_ms.as_ref(), "httpIdleTimeoutMs")?
                .unwrap_or(DEFAULT_HTTP_IDLE_TIMEOUT_MS),
        )
    }

    pub fn set_http_idle_timeout_ms(&self, timeout_ms: f64) -> Result<(), String> {
        if !timeout_ms.is_finite() || timeout_ms < 0.0 {
            return Err(format!("Invalid httpIdleTimeoutMs setting: {timeout_ms}"));
        }
        self.set_global_field("httpIdleTimeoutMs", Value::from(timeout_ms.floor() as u64));
        Ok(())
    }

    pub fn get_provider_retry_settings(&self) -> ResolvedProviderRetrySettings {
        let provider = self
            .settings_snapshot()
            .retry
            .and_then(|retry| retry.provider);
        ResolvedProviderRetrySettings {
            timeout_ms: provider.as_ref().and_then(|provider| provider.timeout_ms),
            max_retries: provider.as_ref().and_then(|provider| provider.max_retries),
            max_retry_delay_ms: provider
                .and_then(|provider| provider.max_retry_delay_ms)
                .unwrap_or(60_000),
        }
    }

    pub fn get_websocket_connect_timeout_ms(&self) -> Result<Option<u64>, String> {
        let settings = self.settings_snapshot();
        parse_timeout_setting(
            settings.websocket_connect_timeout_ms.as_ref(),
            "websocketConnectTimeoutMs",
        )
    }

    pub fn get_hide_thinking_block(&self) -> bool {
        self.settings_snapshot()
            .hide_thinking_block
            .unwrap_or(false)
    }

    pub fn set_hide_thinking_block(&self, hide: bool) {
        self.set_global_field("hideThinkingBlock", Value::from(hide));
    }

    pub fn get_show_cache_miss_notices(&self) -> bool {
        self.settings_snapshot()
            .show_cache_miss_notices
            .unwrap_or(false)
    }

    pub fn set_show_cache_miss_notices(&self, show: bool) {
        self.set_global_field("showCacheMissNotices", Value::from(show));
    }

    pub fn get_external_editor_command(&self) -> String {
        if let Some(editor) = self
            .settings_snapshot()
            .external_editor
            .filter(|editor| !editor.trim().is_empty())
        {
            return editor;
        }
        for variable in ["VISUAL", "EDITOR"] {
            if let Ok(editor) = std::env::var(variable)
                && !editor.is_empty()
            {
                return editor;
            }
        }
        if cfg!(windows) {
            "notepad".to_owned()
        } else {
            "nano".to_owned()
        }
    }

    pub fn get_shell_path(&self) -> Option<String> {
        let shell_path = self.settings_snapshot().shell_path?;
        Some(normalize_setting_path(&shell_path))
    }

    pub fn set_shell_path(&self, path: Option<&str>) {
        self.set_global_field("shellPath", optional_string(path));
    }

    pub fn get_quiet_startup(&self) -> bool {
        self.settings_snapshot().quiet_startup.unwrap_or(false)
    }

    pub fn set_quiet_startup(&self, quiet: bool) {
        self.set_global_field("quietStartup", Value::from(quiet));
    }

    /// Reads the global scope only: a project must not grant itself trust.
    pub fn get_default_project_trust(&self) -> DefaultProjectTrust {
        match as_str(&self.global_snapshot().default_project_trust) {
            Some("always") => DefaultProjectTrust::Always,
            Some("never") => DefaultProjectTrust::Never,
            _ => DefaultProjectTrust::Ask,
        }
    }

    pub fn set_default_project_trust(&self, default_project_trust: DefaultProjectTrust) {
        self.set_global_field("defaultProjectTrust", json_of(&default_project_trust));
    }

    pub fn get_shell_command_prefix(&self) -> Option<String> {
        self.settings_snapshot().shell_command_prefix
    }

    pub fn set_shell_command_prefix(&self, prefix: Option<&str>) {
        self.set_global_field("shellCommandPrefix", optional_string(prefix));
    }

    pub fn get_npm_command(&self) -> Option<Vec<String>> {
        self.settings_snapshot().npm_command
    }

    pub fn set_npm_command(&self, command: Option<&[String]>) {
        self.set_global_field(
            "npmCommand",
            command.map_or(Value::Null, |command| json_of(&command.to_vec())),
        );
    }

    pub fn get_enable_install_telemetry(&self) -> bool {
        self.settings_snapshot()
            .enable_install_telemetry
            .unwrap_or(true)
    }

    pub fn set_enable_install_telemetry(&self, enabled: bool) {
        self.set_global_field("enableInstallTelemetry", Value::from(enabled));
    }

    pub fn get_enable_analytics(&self) -> bool {
        self.settings_snapshot().enable_analytics.unwrap_or(false)
    }

    pub fn get_tracking_id(&self) -> Option<String> {
        self.settings_snapshot().tracking_id
    }

    /// Set the analytics opt-in preference; generates a tracking identifier on
    /// first opt-in.
    pub fn set_enable_analytics(&self, enabled: bool) {
        let mut fields = vec![("enableAnalytics", Value::from(enabled))];
        if enabled && self.global_snapshot().tracking_id.is_none() {
            fields.push(("trackingId", Value::from(uuid::Uuid::new_v4().to_string())));
        }
        self.set_global_fields(&fields);
    }

    pub fn get_packages(&self) -> Vec<PackageSource> {
        self.settings_snapshot().packages.unwrap_or_default()
    }

    pub fn set_packages(&self, packages: &[PackageSource]) {
        self.set_global_field("packages", json_of(&packages.to_vec()));
    }

    pub fn set_project_packages(&self, packages: &[PackageSource]) -> Result<(), SettingsError> {
        self.set_project_field("packages", json_of(&packages.to_vec()))
    }

    pub fn get_skill_paths(&self) -> Vec<String> {
        self.settings_snapshot().skills.unwrap_or_default()
    }

    pub fn set_skill_paths(&self, paths: &[String]) {
        self.set_global_field("skills", json_of(&paths.to_vec()));
    }

    pub fn set_project_skill_paths(&self, paths: &[String]) -> Result<(), SettingsError> {
        self.set_project_field("skills", json_of(&paths.to_vec()))
    }

    pub fn get_prompt_template_paths(&self) -> Vec<String> {
        self.settings_snapshot().prompts.unwrap_or_default()
    }

    pub fn set_prompt_template_paths(&self, paths: &[String]) {
        self.set_global_field("prompts", json_of(&paths.to_vec()));
    }

    pub fn set_project_prompt_template_paths(&self, paths: &[String]) -> Result<(), SettingsError> {
        self.set_project_field("prompts", json_of(&paths.to_vec()))
    }

    pub fn get_theme_paths(&self) -> Vec<String> {
        self.settings_snapshot().themes.unwrap_or_default()
    }

    pub fn set_theme_paths(&self, paths: &[String]) {
        self.set_global_field("themes", json_of(&paths.to_vec()));
    }

    pub fn set_project_theme_paths(&self, paths: &[String]) -> Result<(), SettingsError> {
        self.set_project_field("themes", json_of(&paths.to_vec()))
    }

    pub fn get_enable_skill_commands(&self) -> bool {
        self.settings_snapshot()
            .enable_skill_commands
            .unwrap_or(true)
    }

    pub fn set_enable_skill_commands(&self, enabled: bool) {
        self.set_global_field("enableSkillCommands", Value::from(enabled));
    }

    pub fn get_thinking_budgets(&self) -> Option<ThinkingBudgetsSettings> {
        self.settings_snapshot().thinking_budgets
    }

    pub fn get_show_images(&self) -> bool {
        self.settings_snapshot()
            .terminal
            .and_then(|terminal| terminal.show_images)
            .unwrap_or(true)
    }

    pub fn set_show_images(&self, show: bool) {
        self.set_global_nested_field("terminal", "showImages", Value::from(show));
    }

    pub fn get_image_width_cells(&self) -> u64 {
        let width = self
            .settings_snapshot()
            .terminal
            .and_then(|terminal| terminal.image_width_cells);
        match present(width.as_ref()).and_then(Value::as_f64) {
            Some(width) if width.is_finite() => (width.floor() as i64).max(1) as u64,
            _ => 60,
        }
    }

    pub fn set_image_width_cells(&self, width: f64) {
        let width = (width.floor() as i64).max(1) as u64;
        self.set_global_nested_field("terminal", "imageWidthCells", Value::from(width));
    }

    pub fn get_clear_on_shrink(&self) -> bool {
        // Settings take precedence, then the environment, then false.
        if let Some(clear_on_shrink) = self
            .settings_snapshot()
            .terminal
            .and_then(|terminal| terminal.clear_on_shrink)
        {
            return clear_on_shrink;
        }
        std::env::var("NOTAGENT_CLEAR_ON_SHRINK").is_ok_and(|value| value == "1")
    }

    pub fn set_clear_on_shrink(&self, enabled: bool) {
        self.set_global_nested_field("terminal", "clearOnShrink", Value::from(enabled));
    }

    pub fn get_show_terminal_progress(&self) -> bool {
        self.settings_snapshot()
            .terminal
            .and_then(|terminal| terminal.show_terminal_progress)
            .unwrap_or(false)
    }

    pub fn set_show_terminal_progress(&self, enabled: bool) {
        self.set_global_nested_field("terminal", "showTerminalProgress", Value::from(enabled));
    }

    pub fn get_tui_mode(&self) -> TuiMode {
        match as_str(&self.settings_snapshot().tui_mode) {
            Some("fullscreen") => TuiMode::Fullscreen,
            _ => TuiMode::Regular,
        }
    }

    pub fn set_tui_mode(&self, mode: TuiMode) {
        self.set_global_field("tuiMode", json_of(&mode));
    }

    pub fn get_fullscreen_exit_output(&self) -> FullscreenExitOutput {
        match as_str(&self.settings_snapshot().fullscreen_exit_output) {
            Some("resume-hint") => FullscreenExitOutput::ResumeHint,
            _ => FullscreenExitOutput::Transcript,
        }
    }

    pub fn set_fullscreen_exit_output(&self, output: FullscreenExitOutput) {
        self.set_global_field("fullscreenExitOutput", json_of(&output));
    }

    pub fn get_fullscreen_scrollbar(&self) -> ScrollViewScrollbar {
        match self.settings_snapshot().fullscreen_scrollbar.as_deref() {
            Some("always") => ScrollViewScrollbar::Always,
            Some("hidden") => ScrollViewScrollbar::Hidden,
            _ => ScrollViewScrollbar::Auto,
        }
    }

    pub fn set_fullscreen_scrollbar(&self, mode: ScrollViewScrollbar) {
        self.set_global_field("fullscreenScrollbar", json_of(&mode));
    }

    pub fn get_image_auto_resize(&self) -> bool {
        self.settings_snapshot()
            .images
            .and_then(|images| images.auto_resize)
            .unwrap_or(true)
    }

    pub fn set_image_auto_resize(&self, enabled: bool) {
        self.set_global_nested_field("images", "autoResize", Value::from(enabled));
    }

    pub fn get_block_images(&self) -> bool {
        self.settings_snapshot()
            .images
            .and_then(|images| images.block_images)
            .unwrap_or(false)
    }

    pub fn set_block_images(&self, blocked: bool) {
        self.set_global_nested_field("images", "blockImages", Value::from(blocked));
    }

    pub fn get_enabled_models(&self) -> Option<Vec<String>> {
        self.settings_snapshot().enabled_models
    }

    pub fn set_enabled_models(&self, patterns: Option<&[String]>) {
        self.set_global_field(
            "enabledModels",
            patterns.map_or(Value::Null, |patterns| json_of(&patterns.to_vec())),
        );
    }

    pub fn get_double_escape_action(&self) -> DoubleEscapeAction {
        match as_str(&self.settings_snapshot().double_escape_action) {
            Some("fork") => DoubleEscapeAction::Fork,
            Some("none") => DoubleEscapeAction::None,
            _ => DoubleEscapeAction::Tree,
        }
    }

    pub fn set_double_escape_action(&self, action: DoubleEscapeAction) {
        self.set_global_field("doubleEscapeAction", json_of(&action));
    }

    pub fn get_tree_filter_mode(&self) -> TreeFilterMode {
        match as_str(&self.settings_snapshot().tree_filter_mode) {
            Some("no-tools") => TreeFilterMode::NoTools,
            Some("user-only") => TreeFilterMode::UserOnly,
            Some("labeled-only") => TreeFilterMode::LabeledOnly,
            Some("all") => TreeFilterMode::All,
            _ => TreeFilterMode::Default,
        }
    }

    pub fn set_tree_filter_mode(&self, mode: TreeFilterMode) {
        self.set_global_field("treeFilterMode", json_of(&mode));
    }

    pub fn get_show_hardware_cursor(&self) -> bool {
        self.settings_snapshot()
            .show_hardware_cursor
            .unwrap_or_else(|| {
                std::env::var("NOTAGENT_HARDWARE_CURSOR").is_ok_and(|value| value == "1")
            })
    }

    pub fn set_show_hardware_cursor(&self, enabled: bool) {
        self.set_global_field("showHardwareCursor", Value::from(enabled));
    }

    pub fn get_editor_padding_x(&self) -> u64 {
        as_u64_or(&self.settings_snapshot().editor_padding_x, 0)
    }

    pub fn set_editor_padding_x(&self, padding: f64) {
        let padding = padding.floor().clamp(0.0, 3.0) as u64;
        self.set_global_field("editorPaddingX", Value::from(padding));
    }

    pub fn get_output_pad(&self) -> u64 {
        match present(self.settings_snapshot().output_pad.as_ref()).and_then(Value::as_f64) {
            Some(0.0) => 0,
            _ => 1,
        }
    }

    pub fn set_output_pad(&self, padding: u64) {
        self.set_global_field("outputPad", Value::from(padding));
    }

    pub fn get_autocomplete_max_visible(&self) -> u64 {
        as_u64_or(&self.settings_snapshot().autocomplete_max_visible, 5)
    }

    pub fn set_autocomplete_max_visible(&self, max_visible: f64) {
        let max_visible = max_visible.floor().clamp(3.0, 20.0) as u64;
        self.set_global_field("autocompleteMaxVisible", Value::from(max_visible));
    }

    pub fn get_code_block_indent(&self) -> String {
        self.settings_snapshot()
            .markdown
            .and_then(|markdown| markdown.code_block_indent)
            .unwrap_or_else(|| "  ".to_owned())
    }

    pub fn get_mermaid_rendering_mode(&self) -> MermaidRenderingMode {
        let mode = self
            .settings_snapshot()
            .markdown
            .and_then(|markdown| markdown.mermaid);
        match present(mode.as_ref()).and_then(Value::as_str) {
            Some("off") => MermaidRenderingMode::Off,
            Some("final") => MermaidRenderingMode::Final,
            _ => MermaidRenderingMode::Streaming,
        }
    }

    pub fn set_mermaid_rendering_mode(&self, mode: MermaidRenderingMode) {
        self.set_global_nested_field("markdown", "mermaid", json_of(&mode));
    }

    pub fn get_warnings(&self) -> WarningSettings {
        self.settings_snapshot().warnings.unwrap_or_default()
    }

    pub fn set_warnings(&self, warnings: &WarningSettings) {
        self.set_global_field("warnings", json_of(warnings));
    }
}

/// `normalizePath(value)`; an unusable `file:` URL is kept verbatim rather than
/// failing the accessor (TS throws here, which no caller handles).
fn normalize_setting_path(value: &str) -> String {
    crate::utils::paths::normalize_path_default(value).unwrap_or_else(|_| value.to_owned())
}

fn json_of<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("settings value is serializable")
}

/// `undefined` clears a field; `set_global_field` removes JSON nulls on persist.
fn optional_string(value: Option<&str>) -> Value {
    value.map_or(Value::Null, Value::from)
}

/// The TS `steeringMode`/`followUpMode` accessors return the stored value
/// unchecked; every consumer compares it against `"all"`, so an unknown value
/// behaves like the default here as well.
fn queue_mode(value: &Option<Value>) -> QueueMode {
    match as_str(value) {
        Some("all") => QueueMode::All,
        _ => QueueMode::OneAtATime,
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
