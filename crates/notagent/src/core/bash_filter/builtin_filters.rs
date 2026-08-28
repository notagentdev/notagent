//! The built-in filter documents, embedded at compile time.
//! no build script anywhere and embeds data files with an explicit
//! `include_str!` list instead, the way `core/modes.rs` embeds the built-in
//! modes. The guarantees the build script asserted — the expected file count,
//! one filter per file, no duplicate names — are unit tests in
//! [`super::toml_filter`] rather than build-time panics, so a missing file
//! fails a test instead of a compile.

/// Every built-in filter file, in the alphabetical order the registry reads
/// them: an earlier entry wins when two `match_command` patterns overlap.
pub(super) const BUILTIN_FILTER_FILES: &[(&str, &str)] = &[
    (
        "ansible-playbook.toml",
        include_str!("filters/ansible-playbook.toml"),
    ),
    (
        "basedpyright.toml",
        include_str!("filters/basedpyright.toml"),
    ),
    ("biome.toml", include_str!("filters/biome.toml")),
    (
        "brew-install.toml",
        include_str!("filters/brew-install.toml"),
    ),
    (
        "bundle-install.toml",
        include_str!("filters/bundle-install.toml"),
    ),
    (
        "composer-install.toml",
        include_str!("filters/composer-install.toml"),
    ),
    ("df.toml", include_str!("filters/df.toml")),
    (
        "dotnet-build.toml",
        include_str!("filters/dotnet-build.toml"),
    ),
    ("du.toml", include_str!("filters/du.toml")),
    (
        "fail2ban-client.toml",
        include_str!("filters/fail2ban-client.toml"),
    ),
    ("gcc.toml", include_str!("filters/gcc.toml")),
    ("gcloud.toml", include_str!("filters/gcloud.toml")),
    ("gradle.toml", include_str!("filters/gradle.toml")),
    ("hadolint.toml", include_str!("filters/hadolint.toml")),
    ("helm.toml", include_str!("filters/helm.toml")),
    ("iptables.toml", include_str!("filters/iptables.toml")),
    ("jira.toml", include_str!("filters/jira.toml")),
    ("jj.toml", include_str!("filters/jj.toml")),
    ("jq.toml", include_str!("filters/jq.toml")),
    ("just.toml", include_str!("filters/just.toml")),
    ("liquibase.toml", include_str!("filters/liquibase.toml")),
    ("make.toml", include_str!("filters/make.toml")),
    (
        "markdownlint.toml",
        include_str!("filters/markdownlint.toml"),
    ),
    ("mise.toml", include_str!("filters/mise.toml")),
    ("mix-compile.toml", include_str!("filters/mix-compile.toml")),
    ("mix-format.toml", include_str!("filters/mix-format.toml")),
    ("nx.toml", include_str!("filters/nx.toml")),
    ("ollama.toml", include_str!("filters/ollama.toml")),
    ("oxlint.toml", include_str!("filters/oxlint.toml")),
    ("ping.toml", include_str!("filters/ping.toml")),
    ("pio-run.toml", include_str!("filters/pio-run.toml")),
    (
        "poetry-install.toml",
        include_str!("filters/poetry-install.toml"),
    ),
    ("pre-commit.toml", include_str!("filters/pre-commit.toml")),
    ("ps.toml", include_str!("filters/ps.toml")),
    (
        "pulumi-destroy.toml",
        include_str!("filters/pulumi-destroy.toml"),
    ),
    (
        "pulumi-preview.toml",
        include_str!("filters/pulumi-preview.toml"),
    ),
    (
        "pulumi-refresh.toml",
        include_str!("filters/pulumi-refresh.toml"),
    ),
    (
        "pulumi-stack.toml",
        include_str!("filters/pulumi-stack.toml"),
    ),
    ("pulumi-up.toml", include_str!("filters/pulumi-up.toml")),
    (
        "quarto-render.toml",
        include_str!("filters/quarto-render.toml"),
    ),
    ("rsync.toml", include_str!("filters/rsync.toml")),
    ("shellcheck.toml", include_str!("filters/shellcheck.toml")),
    (
        "shopify-theme.toml",
        include_str!("filters/shopify-theme.toml"),
    ),
    ("skopeo.toml", include_str!("filters/skopeo.toml")),
    ("sops.toml", include_str!("filters/sops.toml")),
    ("spring-boot.toml", include_str!("filters/spring-boot.toml")),
    ("ssh.toml", include_str!("filters/ssh.toml")),
    ("stat.toml", include_str!("filters/stat.toml")),
    ("swift-build.toml", include_str!("filters/swift-build.toml")),
    (
        "systemctl-status.toml",
        include_str!("filters/systemctl-status.toml"),
    ),
    ("task.toml", include_str!("filters/task.toml")),
    (
        "terraform-plan.toml",
        include_str!("filters/terraform-plan.toml"),
    ),
    ("tofu-fmt.toml", include_str!("filters/tofu-fmt.toml")),
    ("tofu-init.toml", include_str!("filters/tofu-init.toml")),
    ("tofu-plan.toml", include_str!("filters/tofu-plan.toml")),
    (
        "tofu-validate.toml",
        include_str!("filters/tofu-validate.toml"),
    ),
    ("trunk-build.toml", include_str!("filters/trunk-build.toml")),
    ("turbo.toml", include_str!("filters/turbo.toml")),
    ("ty.toml", include_str!("filters/ty.toml")),
    ("uv-sync.toml", include_str!("filters/uv-sync.toml")),
    ("xcodebuild.toml", include_str!("filters/xcodebuild.toml")),
    ("yadm.toml", include_str!("filters/yadm.toml")),
    ("yamllint.toml", include_str!("filters/yamllint.toml")),
];

/// The one document the registry parses: a single `schema_version` followed by
/// every filter file, each behind a comment naming its source.
pub(super) fn builtin_document() -> String {
    let mut combined = String::from("schema_version = 1\n\n");
    for (name, content) in BUILTIN_FILTER_FILES {
        combined.push_str(&format!("# --- {name} ---\n"));
        combined.push_str(content);
        combined.push_str("\n\n");
    }
    combined
}
