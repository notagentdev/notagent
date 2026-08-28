use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use notagent::config::{
    InstallEnv, InstallMethod, SelfUpdateCommand, SelfUpdateCommandStep, SelfUpdatePackageTarget,
    detect_install_method, get_self_update_command, get_self_update_unavailable_instruction,
    get_update_instruction,
};

/// PATH is process-global, so the tests that stub package managers run serially.
fn path_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

struct TempDir {
    path: PathBuf,
    original_path: Option<String>,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let directory = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir()
            .expect("temp dir");
        let path = directory.path().to_path_buf();
        let _ = directory.keep();
        Self {
            path,
            original_path: std::env::var("PATH").ok(),
        }
    }

    fn join(&self, relative: &str) -> PathBuf {
        relative
            .split('/')
            .fold(self.path.clone(), |path, part| path.join(part))
    }

    fn create_dir(&self, relative: &str) -> PathBuf {
        let path = self.join(relative);
        std::fs::create_dir_all(&path).expect("creates");
        path
    }

    fn stub_command(&self, bin_dir: &Path, name: &str, script: &str) {
        std::fs::create_dir_all(bin_dir).expect("creates");
        let path = bin_dir.join(name);
        std::fs::write(&path, script).expect("writes");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let previous = self.original_path.clone().unwrap_or_default();
        // SAFETY: `path_lock` keeps the other PATH-dependent tests out.
        unsafe {
            std::env::set_var("PATH", format!("{}:{previous}", bin_dir.to_string_lossy()));
        }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // SAFETY: see `stub_command`.
        unsafe {
            match &self.original_path {
                Some(path) => std::env::set_var("PATH", path),
                None => std::env::remove_var("PATH"),
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o700));
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn answering_script(condition: &str, answer: &Path) -> String {
    format!(
        "#!/bin/sh\nif {condition}; then\n\tprintf '%s\\n' '{}'\n\texit 0\nfi\nexit 1\n",
        answer.to_string_lossy().replace('\'', "'\\''")
    )
}

fn env_at(package_dir: &Path, exec_path: &Path) -> InstallEnv {
    InstallEnv {
        package_dir: package_dir.to_path_buf(),
        exec_path: exec_path.to_path_buf(),
        entrypoint: Some(exec_path.to_path_buf()),
        standalone_binary: false,
    }
}

fn step(command: &str, args: &[&str], display: &str) -> SelfUpdateCommandStep {
    SelfUpdateCommandStep {
        command: command.to_owned(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        display: display.to_owned(),
    }
}

fn command(
    install: SelfUpdateCommandStep,
    steps: Option<Vec<SelfUpdateCommandStep>>,
) -> SelfUpdateCommand {
    let display = match &steps {
        Some(steps) => format!("{} && {}", steps[0].display, steps[1].display),
        None => install.display.clone(),
    };
    SelfUpdateCommand {
        command: install.command,
        args: install.args,
        display,
        steps,
    }
}

/// `createNpmPrefixInstall`
fn npm_prefix_install(prefix: &str) -> (TempDir, PathBuf, PathBuf) {
    let directory = TempDir::new(prefix);
    let package_dir = directory.create_dir("lib/node_modules/@notagentdev/notagent-coding-agent");
    let stub = directory.join("stub-bin");
    let root = directory.join("lib/node_modules");
    directory.stub_command(
        &stub,
        "npm",
        &answering_script("[ \"$#\" -ge 2 ] && [ \"${@: -2:1}\" = \"root\" ]", &root),
    );
    let exec_path = package_dir.join("dist").join("cli.js");
    (directory, package_dir, exec_path)
}

#[test]
fn detects_pnpm_from_windows_pnpm_install_paths() {
    let _guard = path_lock();
    let directory = TempDir::new("notagent-pnpm-windows-");
    let root = directory.create_dir("pnpm/global/5/node_modules");
    directory.stub_command(
        &directory.join("bin"),
        "pnpm",
        &answering_script("[ \"$1\" = \"root\" ] && [ \"$2\" = \"-g\" ]", &root),
    );
    let package_dir = PathBuf::from(
        "C:\\Users\\Admin\\Documents\\pnpm-repository\\global\\5\\.pnpm\\@notagent+coding-agent@0.67.68\\node_modules\\@notagent\\coding-agent",
    );
    let env = env_at(&package_dir, &package_dir.join("dist").join("cli.js"));

    assert_eq!(detect_install_method(&env), InstallMethod::Pnpm);
    assert_eq!(
        get_update_instruction(&env, "@notagent/coding-agent").expect("instruction"),
        "Run: pnpm install -g --ignore-scripts --config.minimumReleaseAge=0 @notagent/coding-agent"
    );
}

#[test]
fn does_not_self_update_unknown_wrapper_installs() {
    let _guard = path_lock();
    let env = env_at(
        Path::new("/usr/local/lib"),
        Path::new("/usr/local/bin/node"),
    );

    assert_eq!(detect_install_method(&env), InstallMethod::Unknown);
    assert_eq!(
        get_self_update_command(
            &env,
            "@notagent/coding-agent",
            None,
            &SelfUpdatePackageTarget::new("@notagent/coding-agent"),
        )
        .expect("no error"),
        None
    );
    assert_eq!(
        get_update_instruction(&env, "@notagent/coding-agent").expect("instruction"),
        "Update @notagent/coding-agent using the package manager, wrapper, or source checkout that provides this installation."
    );
}

#[test]
fn a_standalone_binary_points_at_the_releases_page() {
    let _guard = path_lock();
    let env = InstallEnv {
        package_dir: PathBuf::from("/opt/notagent"),
        exec_path: PathBuf::from("/opt/notagent/notagent"),
        entrypoint: None,
        standalone_binary: true,
    };
    assert_eq!(detect_install_method(&env), InstallMethod::Binary);
    assert_eq!(
        get_update_instruction(&env, "notagent").expect("instruction"),
        "Download from: https://github.com/notagentdev/notagent/releases/latest"
    );
}

#[test]
fn self_updates_npm_installs_from_custom_prefixes() {
    let _guard = path_lock();
    let (directory, package_dir, exec_path) = npm_prefix_install("notagent-prefix-");
    let prefix = directory.path.to_string_lossy().into_owned();
    let env = env_at(&package_dir, &exec_path);

    assert_eq!(detect_install_method(&env), InstallMethod::Npm);
    let install = format!(
        "npm --prefix {prefix} install -g --ignore-scripts --min-release-age=0 @notagent/coding-agent"
    );
    assert_eq!(
        get_self_update_command(
            &env,
            "@notagent/coding-agent",
            None,
            &SelfUpdatePackageTarget::new("@notagent/coding-agent"),
        )
        .expect("no error"),
        Some(command(
            step(
                "npm",
                &[
                    "--prefix",
                    &prefix,
                    "install",
                    "-g",
                    "--ignore-scripts",
                    "--min-release-age=0",
                    "@notagent/coding-agent",
                ],
                &install,
            ),
            None,
        ))
    );
}

#[test]
fn self_updates_exact_npm_versions_without_uninstalling_the_current_package() {
    let _guard = path_lock();
    let (directory, package_dir, exec_path) = npm_prefix_install("notagent-prefix-");
    let prefix = directory.path.to_string_lossy().into_owned();
    let env = env_at(&package_dir, &exec_path);

    let target = SelfUpdatePackageTarget::with_spec(
        "@notagent/coding-agent",
        "@notagent/coding-agent@1.2.3",
    );
    let display = format!(
        "npm --prefix {prefix} install -g --ignore-scripts --min-release-age=0 @notagent/coding-agent@1.2.3"
    );
    assert_eq!(
        get_self_update_command(&env, "@notagent/coding-agent", None, &target).expect("no error"),
        Some(command(
            step(
                "npm",
                &[
                    "--prefix",
                    &prefix,
                    "install",
                    "-g",
                    "--ignore-scripts",
                    "--min-release-age=0",
                    "@notagent/coding-agent@1.2.3",
                ],
                &display,
            ),
            None,
        ))
    );
}

#[test]
fn self_updates_renamed_packages_from_the_current_install_prefix() {
    let _guard = path_lock();
    let (directory, package_dir, exec_path) = npm_prefix_install("notagent-prefix-");
    let prefix = directory.path.to_string_lossy().into_owned();
    let env = env_at(&package_dir, &exec_path);

    let uninstall = format!("npm --prefix {prefix} uninstall -g @mariozechner/pi-coding-agent");
    let install = format!(
        "npm --prefix {prefix} install -g --ignore-scripts --min-release-age=0 @new-scope/notagent"
    );
    let install_step = step(
        "npm",
        &[
            "--prefix",
            &prefix,
            "install",
            "-g",
            "--ignore-scripts",
            "--min-release-age=0",
            "@new-scope/notagent",
        ],
        &install,
    );
    assert_eq!(
        get_self_update_command(
            &env,
            "@mariozechner/pi-coding-agent",
            None,
            &SelfUpdatePackageTarget::new("@new-scope/notagent"),
        )
        .expect("no error"),
        Some(command(
            install_step.clone(),
            Some(vec![
                step(
                    "npm",
                    &[
                        "--prefix",
                        &prefix,
                        "uninstall",
                        "-g",
                        "@mariozechner/pi-coding-agent"
                    ],
                    &uninstall,
                ),
                install_step,
            ]),
        ))
    );
}

#[test]
fn self_update_respects_configured_and_empty_npm_commands() {
    let _guard = path_lock();
    let (directory, package_dir, exec_path) = npm_prefix_install("notagent-prefix-");
    let prefix = directory.path.to_string_lossy().into_owned();
    let env = env_at(&package_dir, &exec_path);
    let expected = vec![
        "--prefix".to_owned(),
        prefix.clone(),
        "install".to_owned(),
        "-g".to_owned(),
        "--ignore-scripts".to_owned(),
        "--min-release-age=0".to_owned(),
        "@notagent/coding-agent".to_owned(),
    ];

    let configured = ["npm".to_owned(), "--prefix".to_owned(), prefix.clone()];
    let command = get_self_update_command(
        &env,
        "@notagent/coding-agent",
        Some(&configured),
        &SelfUpdatePackageTarget::new("@notagent/coding-agent"),
    )
    .expect("no error")
    .expect("command");
    assert_eq!(command.args, expected);

    // An empty npmCommand counts as unset, so the prefix is inferred instead.
    let command = get_self_update_command(
        &env,
        "@notagent/coding-agent",
        Some(&[]),
        &SelfUpdatePackageTarget::new("@notagent/coding-agent"),
    )
    .expect("no error")
    .expect("command");
    assert_eq!(command.args, expected);
}

#[test]
fn quotes_npm_self_update_display_paths() {
    let _guard = path_lock();
    let (directory, package_dir, exec_path) = npm_prefix_install("notagent prefix ");
    let prefix = directory.path.to_string_lossy().into_owned();
    let env = env_at(&package_dir, &exec_path);

    let command = get_self_update_command(
        &env,
        "@notagent/coding-agent",
        None,
        &SelfUpdatePackageTarget::new("@notagent/coding-agent"),
    )
    .expect("no error")
    .expect("command");
    assert_eq!(
        command.display,
        format!(
            "npm --prefix \"{prefix}\" install -g --ignore-scripts --min-release-age=0 @notagent/coding-agent"
        )
    );
}

#[test]
fn does_not_infer_windows_npm_custom_prefixes_from_package_paths() {
    let _guard = path_lock();
    let package_dir =
        PathBuf::from("C:\\Users\\Admin\\npm prefix\\node_modules\\@notagent\\coding-agent");
    let env = env_at(&package_dir, &package_dir.join("dist").join("cli.js"));

    assert_eq!(detect_install_method(&env), InstallMethod::Npm);
    assert_eq!(
        get_update_instruction(&env, "@notagent/coding-agent").expect("instruction"),
        "Run: npm install -g --ignore-scripts --min-release-age=0 @notagent/coding-agent"
    );
}

#[test]
fn self_updates_bun_global_installs_from_bun_pm_bin() {
    let _guard = path_lock();
    let directory = TempDir::new("notagent-bun-");
    let package_dir =
        directory.create_dir(".bun/install/global/node_modules/@notagentdev/notagent-coding-agent");
    let bun_bin = directory.join(".bun/bin");
    directory.stub_command(
        &bun_bin,
        "bun",
        &answering_script(
            "[ \"$1\" = \"pm\" ] && [ \"$2\" = \"bin\" ] && [ \"$3\" = \"-g\" ]",
            &bun_bin,
        ),
    );
    let env = env_at(&package_dir, &package_dir.join("dist").join("cli.js"));

    assert_eq!(detect_install_method(&env), InstallMethod::Bun);
    let display = "bun install -g --ignore-scripts --minimum-release-age=0 @notagent/coding-agent";
    assert_eq!(
        get_self_update_command(
            &env,
            "@notagent/coding-agent",
            None,
            &SelfUpdatePackageTarget::new("@notagent/coding-agent"),
        )
        .expect("no error"),
        Some(command(
            step(
                "bun",
                &[
                    "install",
                    "-g",
                    "--ignore-scripts",
                    "--minimum-release-age=0",
                    "@notagent/coding-agent",
                ],
                display,
            ),
            None,
        ))
    );
}

#[test]
fn self_updates_renamed_pnpm_global_installs_by_removing_the_old_package_first() {
    let _guard = path_lock();
    let directory = TempDir::new("notagent-pnpm-");
    let root = directory.create_dir("pnpm/global/5/node_modules");
    let package_dir =
        directory.create_dir("pnpm/global/5/node_modules/@mariozechner/notagent-coding-agent");
    directory.stub_command(
        &directory.join("bin"),
        "pnpm",
        &answering_script("[ \"$1\" = \"root\" ] && [ \"$2\" = \"-g\" ]", &root),
    );
    let exec_path = directory.join(
        "pnpm/global/5/node_modules/.pnpm/@mariozechner+notagent-coding-agent@0.0.0/node_modules/@mariozechner/notagent-coding-agent/dist/cli.js",
    );
    let env = env_at(&package_dir, &exec_path);

    assert_eq!(detect_install_method(&env), InstallMethod::Pnpm);
    let install_step = step(
        "pnpm",
        &[
            "install",
            "-g",
            "--ignore-scripts",
            "--config.minimumReleaseAge=0",
            "@new-scope/notagent",
        ],
        "pnpm install -g --ignore-scripts --config.minimumReleaseAge=0 @new-scope/notagent",
    );
    assert_eq!(
        get_self_update_command(
            &env,
            "@mariozechner/pi-coding-agent",
            None,
            &SelfUpdatePackageTarget::new("@new-scope/notagent"),
        )
        .expect("no error"),
        Some(command(
            install_step.clone(),
            Some(vec![
                step(
                    "pnpm",
                    &["remove", "-g", "@mariozechner/pi-coding-agent"],
                    "pnpm remove -g @mariozechner/pi-coding-agent",
                ),
                install_step,
            ]),
        ))
    );
}

#[test]
fn self_updates_pnpm_v11_global_installs_resolved_through_the_store() {
    let _guard = path_lock();
    let directory = TempDir::new("notagent-pnpm11-");
    let root = directory.join("Library/pnpm/global/v11");
    let global_package_dir = directory.create_dir(
        "Library/pnpm/global/v11/11e9a/node_modules/@notagentdev/notagent-coding-agent",
    );
    let store_package_dir = directory.create_dir(
        "Library/pnpm/store/v11/links/@notagentdev/notagent-coding-agent/0.75.0/hash/node_modules/@notagentdev/notagent-coding-agent",
    );
    std::fs::write(global_package_dir.join("package.json"), "{}").expect("writes");
    directory.stub_command(
        &directory.join("bin"),
        "pnpm",
        &answering_script("[ \"$1\" = \"root\" ] && [ \"$2\" = \"-g\" ]", &root),
    );
    // The entrypoint lives in the global install, the package dir in the store.
    let env = InstallEnv {
        package_dir: store_package_dir.clone(),
        exec_path: store_package_dir.join("dist").join("cli.js"),
        entrypoint: Some(global_package_dir.join("dist").join("cli.js")),
        standalone_binary: false,
    };

    assert_eq!(detect_install_method(&env), InstallMethod::Pnpm);
    let display =
        "pnpm install -g --ignore-scripts --config.minimumReleaseAge=0 @notagent/coding-agent";
    assert_eq!(
        get_self_update_command(
            &env,
            "@notagent/coding-agent",
            None,
            &SelfUpdatePackageTarget::new("@notagent/coding-agent"),
        )
        .expect("no error"),
        Some(command(
            step(
                "pnpm",
                &[
                    "install",
                    "-g",
                    "--ignore-scripts",
                    "--config.minimumReleaseAge=0",
                    "@notagent/coding-agent",
                ],
                display,
            ),
            None,
        ))
    );
}

#[test]
fn self_updates_renamed_yarn_global_installs_by_removing_the_old_package_first() {
    let _guard = path_lock();
    let directory = TempDir::new("notagent-yarn-");
    let global_dir = directory.join("yarn/global");
    let package_dir =
        directory.create_dir("yarn/global/node_modules/@mariozechner/notagent-coding-agent");
    directory.stub_command(
        &directory.join("bin"),
        "yarn",
        &answering_script(
            "[ \"$1\" = \"global\" ] && [ \"$2\" = \"dir\" ]",
            &global_dir,
        ),
    );
    let exec_path =
        directory.join("yarn/global/.yarn/@mariozechner/notagent-coding-agent/dist/cli.js");
    let env = env_at(&package_dir, &exec_path);

    assert_eq!(detect_install_method(&env), InstallMethod::Yarn);
    let install_step = step(
        "yarn",
        &["global", "add", "--ignore-scripts", "@new-scope/notagent"],
        "yarn global add --ignore-scripts @new-scope/notagent",
    );
    assert_eq!(
        get_self_update_command(
            &env,
            "@mariozechner/pi-coding-agent",
            None,
            &SelfUpdatePackageTarget::new("@new-scope/notagent"),
        )
        .expect("no error"),
        Some(command(
            install_step.clone(),
            Some(vec![
                step(
                    "yarn",
                    &["global", "remove", "@mariozechner/pi-coding-agent"],
                    "yarn global remove @mariozechner/pi-coding-agent",
                ),
                install_step,
            ]),
        ))
    );
}

#[test]
fn self_updates_renamed_bun_global_installs_by_removing_the_old_package_first() {
    let _guard = path_lock();
    let directory = TempDir::new("notagent-bun-rename-");
    let package_dir = directory
        .create_dir(".bun/install/global/node_modules/@mariozechner/notagent-coding-agent");
    let bun_bin = directory.join(".bun/bin");
    directory.stub_command(
        &bun_bin,
        "bun",
        &answering_script(
            "[ \"$1\" = \"pm\" ] && [ \"$2\" = \"bin\" ] && [ \"$3\" = \"-g\" ]",
            &bun_bin,
        ),
    );
    let env = env_at(&package_dir, &package_dir.join("dist").join("cli.js"));

    let install_step = step(
        "bun",
        &[
            "install",
            "-g",
            "--ignore-scripts",
            "--minimum-release-age=0",
            "@new-scope/notagent",
        ],
        "bun install -g --ignore-scripts --minimum-release-age=0 @new-scope/notagent",
    );
    assert_eq!(
        get_self_update_command(
            &env,
            "@mariozechner/pi-coding-agent",
            None,
            &SelfUpdatePackageTarget::new("@new-scope/notagent"),
        )
        .expect("no error"),
        Some(command(
            install_step.clone(),
            Some(vec![
                step(
                    "bun",
                    &["uninstall", "-g", "@mariozechner/pi-coding-agent"],
                    "bun uninstall -g @mariozechner/pi-coding-agent",
                ),
                install_step,
            ]),
        ))
    );
}

#[cfg(unix)]
#[test]
fn does_not_self_update_when_the_npm_install_path_is_not_writable() {
    use std::os::unix::fs::PermissionsExt;
    let _guard = path_lock();
    let (_directory, package_dir, exec_path) = npm_prefix_install("notagent-prefix-");
    std::fs::set_permissions(&package_dir, std::fs::Permissions::from_mode(0o500)).expect("chmod");
    let env = env_at(&package_dir, &exec_path);

    let target = SelfUpdatePackageTarget::new("@notagent/coding-agent");
    assert_eq!(
        get_self_update_command(&env, "@notagent/coding-agent", None, &target).expect("no error"),
        None
    );
    let instruction =
        get_self_update_unavailable_instruction(&env, "@notagent/coding-agent", None, &target)
            .expect("instruction");
    assert!(
        instruction.contains("the install path is not writable"),
        "{instruction}"
    );
    std::fs::set_permissions(&package_dir, std::fs::Permissions::from_mode(0o700)).expect("chmod");
}
