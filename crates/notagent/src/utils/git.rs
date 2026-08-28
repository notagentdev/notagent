/// Parsed git URL information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSource {
    /// Clone URL (always valid for `git clone`, without ref suffix).
    pub repo: String,
    /// Git host domain (e.g. `github.com`).
    pub host: String,
    /// Repository path (e.g. `user/repo`).
    pub path: String,
    /// Git ref (branch, tag, commit) if specified.
    pub ref_name: Option<String>,
    /// True if a ref was specified (the package is not auto-updated).
    pub pinned: bool,
}

struct SplitRef {
    repo: String,
    ref_name: Option<String>,
}

/// `url.match(/^git@([^:]+):(.+)$/)`
fn scp_like(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("git@")?;
    let colon = rest.find(':')?;
    let host = &rest[..colon];
    let path = &rest[colon + 1..];
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some((host, path))
}

/// A minimal `new URL(...)` for the four schemes this module accepts.
struct ParsedUrl {
    scheme: String,
    authority: String,
    hostname: String,
    path: String,
}

fn parse_url(url: &str) -> Option<ParsedUrl> {
    let separator = url.find("://")?;
    let scheme = url[..separator].to_lowercase();
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+')
    {
        return None;
    }
    let rest = &url[separator + 3..];
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let path = &rest[end..];
    let host_part = authority.rsplit('@').next().unwrap_or(authority);
    let hostname = host_part
        .split(':')
        .next()
        .unwrap_or(host_part)
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_lowercase();
    if hostname.is_empty() {
        return None;
    }
    Some(ParsedUrl {
        scheme,
        authority: authority.to_owned(),
        hostname,
        path: path.to_owned(),
    })
}

/// `splitRef(url)`
fn split_ref(url: &str) -> SplitRef {
    let unchanged = || SplitRef {
        repo: url.to_owned(),
        ref_name: None,
    };

    if let Some((host, path_with_maybe_ref)) = scp_like(url) {
        let Some(separator) = path_with_maybe_ref.find('@') else {
            return unchanged();
        };
        let repo_path = &path_with_maybe_ref[..separator];
        let ref_name = &path_with_maybe_ref[separator + 1..];
        if repo_path.is_empty() || ref_name.is_empty() {
            return unchanged();
        }
        return SplitRef {
            repo: format!("git@{host}:{repo_path}"),
            ref_name: Some(ref_name.to_owned()),
        };
    }

    if url.contains("://") {
        let Some(parsed) = parse_url(url) else {
            return unchanged();
        };
        let path_with_maybe_ref = parsed.path.trim_start_matches('/');
        let Some(separator) = path_with_maybe_ref.find('@') else {
            return unchanged();
        };
        let repo_path = &path_with_maybe_ref[..separator];
        let ref_name = &path_with_maybe_ref[separator + 1..];
        if repo_path.is_empty() || ref_name.is_empty() {
            return unchanged();
        }
        let repo = format!("{}://{}/{repo_path}", parsed.scheme, parsed.authority);
        return SplitRef {
            repo: repo.trim_end_matches('/').to_owned(),
            ref_name: Some(ref_name.to_owned()),
        };
    }

    let Some(slash_index) = url.find('/') else {
        return unchanged();
    };
    let host = &url[..slash_index];
    let path_with_maybe_ref = &url[slash_index + 1..];
    let Some(separator) = path_with_maybe_ref.find('@') else {
        return unchanged();
    };
    let repo_path = &path_with_maybe_ref[..separator];
    let ref_name = &path_with_maybe_ref[separator + 1..];
    if repo_path.is_empty() || ref_name.is_empty() {
        return unchanged();
    }
    SplitRef {
        repo: format!("{host}/{repo_path}"),
        ref_name: Some(ref_name.to_owned()),
    }
}

/// `decodeURIComponent(value)`; `None` when the sequence is malformed.
fn decode_for_validation(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

/// `hasUnsafeGitInstallPart(value, allowSlash)`
fn has_unsafe_git_install_part(value: &str, allow_slash: bool) -> bool {
    let Some(decoded) = decode_for_validation(value) else {
        return true;
    };
    for candidate in [value, decoded.as_str()] {
        if candidate.contains('\0') || candidate.contains('\\') || candidate.starts_with('/') {
            return true;
        }
        if !allow_slash && candidate.contains('/') {
            return true;
        }
        if candidate.split('/').any(|segment| segment == "..") {
            return true;
        }
    }
    false
}

/// `buildGitSource(args)`
fn build_git_source(
    repo: &str,
    host: &str,
    path: &str,
    ref_name: Option<&str>,
) -> Option<GitSource> {
    if path.starts_with('/') {
        return None;
    }
    let normalized_path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_start_matches('/')
        .to_owned();
    if host.is_empty() || normalized_path.is_empty() || normalized_path.split('/').count() < 2 {
        return None;
    }
    if has_unsafe_git_install_part(host, false)
        || has_unsafe_git_install_part(&normalized_path, true)
    {
        return None;
    }
    Some(GitSource {
        repo: repo.to_owned(),
        host: host.to_owned(),
        path: normalized_path,
        ref_name: ref_name.map(str::to_owned),
        pinned: ref_name.is_some(),
    })
}

/// `parseGenericGitUrl(url)`
fn parse_generic_git_url(url: &str) -> Option<GitSource> {
    let split = split_ref(url);
    let repo_without_ref = split.repo.clone();
    let mut repo = repo_without_ref.clone();
    let host;
    let path;

    if let Some((scp_host, scp_path)) = scp_like(&repo_without_ref) {
        host = scp_host.to_owned();
        path = scp_path.to_owned();
    } else if ["https://", "http://", "ssh://", "git://"]
        .iter()
        .any(|prefix| repo_without_ref.starts_with(prefix))
    {
        let parsed = parse_url(&repo_without_ref)?;
        host = parsed.hostname;
        path = parsed.path.trim_start_matches('/').to_owned();
    } else {
        let slash_index = repo_without_ref.find('/')?;
        host = repo_without_ref[..slash_index].to_owned();
        path = repo_without_ref[slash_index + 1..].to_owned();
        if !host.contains('.') && host != "localhost" {
            return None;
        }
        repo = format!("https://{repo_without_ref}");
    }

    build_git_source(&repo, &host, &path, split.ref_name.as_deref())
}

/// `hostedGitInfo.fromUrl(candidate)` restricted to the reachable default hosts.
struct HostedInfo {
    domain: String,
    user: String,
    project: String,
    committish: Option<String>,
}

fn known_domain(host: &str) -> Option<&'static str> {
    match host.trim_start_matches("www.") {
        "github.com" => Some("github.com"),
        "gitlab.com" => Some("gitlab.com"),
        "bitbucket.org" => Some("bitbucket.org"),
        "gist.github.com" => Some("gist.github.com"),
        _ => None,
    }
}

/// The `#committish` suffix `hosted-git-info` understands.
fn split_committish(value: &str) -> (&str, Option<&str>) {
    match value.split_once('#') {
        Some((head, committish)) if !committish.is_empty() => (head, Some(committish)),
        _ => (value, None),
    }
}

fn hosted_from_url(candidate: &str) -> Option<HostedInfo> {
    let (candidate, committish) = split_committish(candidate);
    let (domain, path) = if let Some((host, path)) = scp_like(candidate) {
        (known_domain(&host.to_lowercase())?, path.to_owned())
    } else if candidate.contains("://") {
        let parsed = parse_url(candidate)?;
        if !matches!(parsed.scheme.as_str(), "http" | "https" | "ssh" | "git") {
            return None;
        }
        (
            known_domain(&parsed.hostname)?,
            parsed.path.trim_start_matches('/').to_owned(),
        )
    } else {
        // Bare shorthand: `user/repo` is github, `host/user/repo` needs a known host.
        match candidate.split('/').count() {
            2 => ("github.com", candidate.to_owned()),
            _ => {
                let (host, rest) = candidate.split_once('/')?;
                (known_domain(&host.to_lowercase())?, rest.to_owned())
            }
        }
    };

    let path = path.trim_end_matches('/');
    let mut segments = path.split('/');
    let user = segments.next()?.to_owned();
    let project = segments.next()?.to_owned();
    if segments.next().is_some() {
        return None;
    }
    if user.is_empty() || project.is_empty() {
        return None;
    }
    Some(HostedInfo {
        domain: domain.to_owned(),
        user,
        project: project.strip_suffix(".git").unwrap_or(&project).to_owned(),
        committish: committish.map(str::to_owned),
    })
}

/// Parse a git source into a [`GitSource`].
/// Rules:
/// - With a `git:` prefix, accept all historical shorthand forms.
/// - Without it, only accept explicit protocol URLs.
pub fn parse_git_url(source: &str) -> Option<GitSource> {
    let trimmed = source.trim();
    let has_git_prefix = trimmed.starts_with("git:");
    let url = if has_git_prefix {
        trimmed[4..].trim()
    } else {
        trimmed
    };

    if !has_git_prefix {
        let lower = url.to_lowercase();
        if !["https://", "http://", "ssh://", "git://"]
            .iter()
            .any(|prefix| lower.starts_with(prefix))
        {
            return None;
        }
    }

    let split = split_ref(url);

    let hosted_candidates: Vec<String> = split
        .ref_name
        .as_ref()
        .map(|ref_name| format!("{}#{ref_name}", split.repo))
        .into_iter()
        .chain(std::iter::once(url.to_owned()))
        .collect();
    for candidate in &hosted_candidates {
        if let Some(info) = hosted_from_url(candidate) {
            if split.ref_name.is_some() && info.project.contains('@') {
                continue;
            }
            let use_https_prefix = !["http://", "https://", "ssh://", "git://", "git@"]
                .iter()
                .any(|prefix| split.repo.starts_with(prefix));
            let repo = if use_https_prefix {
                format!("https://{}", split.repo)
            } else {
                split.repo.clone()
            };
            let ref_name = info.committish.clone().or_else(|| split.ref_name.clone());
            return build_git_source(
                &repo,
                &info.domain,
                &format!("{}/{}", info.user, info.project),
                ref_name.as_deref(),
            );
        }
    }

    let https_candidates: Vec<String> = split
        .ref_name
        .as_ref()
        .map(|ref_name| format!("https://{}#{ref_name}", split.repo))
        .into_iter()
        .chain(std::iter::once(format!("https://{url}")))
        .collect();
    for candidate in &https_candidates {
        if let Some(info) = hosted_from_url(candidate) {
            if split.ref_name.is_some() && info.project.contains('@') {
                continue;
            }
            let ref_name = info.committish.clone().or_else(|| split.ref_name.clone());
            return build_git_source(
                &format!("https://{}", split.repo),
                &info.domain,
                &format!("{}/{}", info.user, info.project),
                ref_name.as_deref(),
            );
        }
    }

    parse_generic_git_url(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(source: &str) -> GitSource {
        parse_git_url(source).unwrap_or_else(|| panic!("expected a git source for {source}"))
    }

    #[test]
    fn parses_protocol_urls_without_a_git_prefix() {
        let result = parsed("https://github.com/user/repo");
        assert_eq!(result.host, "github.com");
        assert_eq!(result.path, "user/repo");
        assert_eq!(result.repo, "https://github.com/user/repo");

        let result = parsed("ssh://git@github.com/user/repo");
        assert_eq!(result.host, "github.com");
        assert_eq!(result.path, "user/repo");
        assert_eq!(result.repo, "ssh://git@github.com/user/repo");

        let result = parsed("https://github.com/user/repo@v1.0.0");
        assert_eq!(result.host, "github.com");
        assert_eq!(result.path, "user/repo");
        assert_eq!(result.ref_name.as_deref(), Some("v1.0.0"));
        assert_eq!(result.repo, "https://github.com/user/repo");
    }

    #[test]
    fn parses_shorthand_urls_only_with_a_git_prefix() {
        let result = parsed("git:git@github.com:user/repo");
        assert_eq!(result.host, "github.com");
        assert_eq!(result.path, "user/repo");
        assert_eq!(result.repo, "git@github.com:user/repo");

        let result = parsed("git:github.com/user/repo");
        assert_eq!(result.host, "github.com");
        assert_eq!(result.path, "user/repo");
        assert_eq!(result.repo, "https://github.com/user/repo");

        let result = parsed("git:git@github.com:user/repo@v1.0.0");
        assert_eq!(result.host, "github.com");
        assert_eq!(result.path, "user/repo");
        assert_eq!(result.ref_name.as_deref(), Some("v1.0.0"));
        assert_eq!(result.repo, "git@github.com:user/repo");
    }

    #[test]
    fn rejects_unsafe_git_install_path_inputs() {
        for source in [
            "git:git@evil.example:../../victim/repo",
            "https://evil.example/..%2F..%2Fvictim/repo",
            "https://evil.example/..%2F..%2Fvictim/repo%",
            "git:git@evil.example:/absolute/repo",
            "git:git@evil.example:user\\repo/name",
            "git:git@evil.example:user/repo\0name",
        ] {
            assert_eq!(parse_git_url(source), None, "expected null for {source}");
        }
    }

    #[test]
    fn rejects_shorthand_without_a_git_prefix() {
        assert_eq!(parse_git_url("git@github.com:user/repo"), None);
        assert_eq!(parse_git_url("github.com/user/repo"), None);
        assert_eq!(parse_git_url("user/repo"), None);
    }

    #[test]
    fn parses_the_hosts_the_docs_list() {
        assert_eq!(parsed("https://github.com/user/repo.git").path, "user/repo");
        assert_eq!(parsed("https://gitlab.com/user/repo").host, "gitlab.com");
        assert_eq!(
            parsed("https://bitbucket.org/user/repo").host,
            "bitbucket.org"
        );
        assert_eq!(
            parsed("https://codeberg.org/user/repo").host,
            "codeberg.org"
        );
        assert_eq!(parsed("git:https://github.com/user/repo").path, "user/repo");
        let with_ref = parsed("https://github.com/user/repo@feature/branch");
        assert_eq!(with_ref.ref_name.as_deref(), Some("feature/branch"));
        assert!(with_ref.pinned);
    }
}
