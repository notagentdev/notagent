use std::sync::Arc;

use futures::future::BoxFuture;

use super::settings_manager::DefaultProjectTrust;
use super::trust_manager::{
    ProjectTrustOption, ProjectTrustStore, TrustStoreError, get_project_trust_options,
    has_trust_requiring_project_resources,
};
use crate::config::CONFIG_DIR_NAME;

/// How the app was started, which decides whether there is anyone to ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Interactive,
    Print,
    Json,
    Rpc,
}

/// Presents the prompt and returns the chosen label, or `None` when the user
/// dismissed it.
pub type ProjectTrustSelect =
    Arc<dyn Fn(String, Vec<String>) -> BoxFuture<'static, Option<String>> + Send + Sync>;

/// What the host can do about trust: nothing at all, or ask.
#[derive(Clone, Default)]
pub struct ProjectTrustContext {
    pub has_ui: bool,
    pub select: Option<ProjectTrustSelect>,
}

pub struct ResolveProjectTrustedOptions<'a> {
    pub cwd: &'a str,
    pub trust_store: &'a ProjectTrustStore,
    pub trust_override: Option<bool>,
    pub default_project_trust: Option<DefaultProjectTrust>,
    pub project_trust_context: ProjectTrustContext,
}

fn format_project_trust_prompt(cwd: &str) -> String {
    format!(
        "Trust project folder?\n{cwd}\n\nThis allows notagent to load {CONFIG_DIR_NAME} settings and resources, install missing project packages, and execute project extensions."
    )
}

async fn select_project_trust_option(
    cwd: &str,
    context: &ProjectTrustContext,
) -> Option<ProjectTrustOption> {
    let options = get_project_trust_options(cwd, true);
    let select = context.select.as_ref()?;
    let labels = options.iter().map(|option| option.label.clone()).collect();
    let selected = select(format_project_trust_prompt(cwd), labels).await?;
    options.into_iter().find(|option| option.label == selected)
}

fn save_project_trust_prompt_result(
    trust_store: &ProjectTrustStore,
    result: &ProjectTrustOption,
) -> Result<(), TrustStoreError> {
    if result.updates.is_empty() {
        return Ok(());
    }
    trust_store.set_many(&result.updates)
}

pub async fn resolve_project_trusted(
    options: ResolveProjectTrustedOptions<'_>,
) -> Result<bool, TrustStoreError> {
    if let Some(trust_override) = options.trust_override {
        return Ok(trust_override);
    }
    if !has_trust_requiring_project_resources(options.cwd) {
        return Ok(true);
    }

    if let Some(decision) = options.trust_store.get(options.cwd)? {
        return Ok(decision);
    }

    match options
        .default_project_trust
        .unwrap_or(DefaultProjectTrust::Ask)
    {
        DefaultProjectTrust::Always => return Ok(true),
        DefaultProjectTrust::Never => return Ok(false),
        DefaultProjectTrust::Ask => {}
    }

    if !options.project_trust_context.has_ui {
        return Ok(false);
    }

    let Some(selected) =
        select_project_trust_option(options.cwd, &options.project_trust_context).await
    else {
        return Ok(false);
    };
    save_project_trust_prompt_result(options.trust_store, &selected)?;
    Ok(selected.trusted)
}
