/// `getPiUserAgent(version)`
/// Deviation (class 4): the runtime token names the Rust distribution instead of
/// `bun/<v>` or `node/<v>` — there is no Node runtime behind this binary.
pub fn get_pi_user_agent(version: &str) -> String {
    format!(
        "notagent/{version} ({}; rust/{}; {})",
        platform(),
        rustc_version(),
        arch()
    )
}

/// `process.platform`
fn platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

/// `process.arch`
fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
}

fn rustc_version() -> &'static str {
    // The crate version stands in for the runtime version: a Rust binary has no
    // separately versioned runtime to report.
    crate::config::VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_the_three_tokens() {
        let agent = get_pi_user_agent("1.2.3");
        assert!(agent.starts_with("notagent/1.2.3 ("));
        assert!(agent.ends_with(&format!("; {})", arch())));
        assert!(agent.contains("rust/"));
    }
}
