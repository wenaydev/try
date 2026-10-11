//! Git URI parsing for `try clone` and the URL shorthand.

use crate::naming;

#[derive(Debug, PartialEq, Eq)]
pub struct GitUri {
    pub host: String,
    pub user: String,
    pub repo: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PullRequest {
    pub user: String,
    pub repo: String,
    pub number: String,
    /// Clone URL of the base repository.
    pub clone_url: String,
}

fn basename(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

fn uri(host: &str, user: &str, repo: &str) -> Option<GitUri> {
    if host.is_empty() || user.is_empty() || repo.is_empty() {
        return None;
    }
    Some(GitUri {
        host: host.into(),
        user: user.into(),
        repo: repo.into(),
    })
}

/// Extract host/user/repo from the URL shapes git accepts:
///
/// * `https://host/user/repo[.git]`
/// * `git@host:user/repo[.git]` (nested paths use the last segment)
/// * `ssh://user@host[:port]/user/path/repo[.git]`
/// * `user@host:path/to/repo[.git]` (SCP style)
pub fn parse_git_uri(input: &str) -> Option<GitUri> {
    let s = input.strip_suffix(".git").unwrap_or(input);

    if let Some(rest) = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
    {
        let mut p = rest.split('/');
        return uri(p.next()?, p.next()?, p.next()?);
    }

    if let Some(rest) = s.strip_prefix("ssh://") {
        let (userinfo, rest) = rest.split_once('@')?;
        if userinfo.is_empty() || userinfo.contains('/') {
            return None;
        }
        let (host, rest) = rest.split_once('/')?;
        let (user, path) = rest.split_once('/')?;
        return uri(host, user, basename(path));
    }

    if let Some(rest) = s.strip_prefix("git@") {
        if let Some((host, path)) = rest.split_once(':') {
            if let Some((user, tail)) = path.split_once('/') {
                if !host.is_empty() && !user.is_empty() && !tail.is_empty() {
                    let first = tail.split('/').next().unwrap_or("");
                    let repo = if host == "github.com" && !first.is_empty() {
                        first
                    } else {
                        basename(tail)
                    };
                    return uri(host, user, repo);
                }
            }
        }
    }

    // SCP style: user@host:path
    let (user, rest) = s.split_once('@')?;
    if user.is_empty() || user.contains(['/', ':']) {
        return None;
    }
    let (host, path) = rest.split_once(':')?;
    if path.is_empty() {
        return None;
    }
    uri(host, user, basename(path))
}

/// Recognise `https://github.com/user/repo/pull/123`.
pub fn github_pr(input: &str) -> Option<PullRequest> {
    let rest = input
        .strip_prefix("https://")
        .or_else(|| input.strip_prefix("http://"))?;
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let rest = rest.strip_prefix("github.com/")?;

    let parts: Vec<&str> = rest.split('/').collect();
    match parts.as_slice() {
        [user, repo, "pull", n] | [user, repo, "pull", n, ""]
            if !user.is_empty()
                && !repo.is_empty()
                && !n.is_empty()
                && n.bytes().all(|b| b.is_ascii_digit()) =>
        {
            let repo = repo.strip_suffix(".git").unwrap_or(repo);
            Some(PullRequest {
                user: (*user).into(),
                repo: repo.into(),
                number: (*n).into(),
                clone_url: format!("https://github.com/{user}/{repo}.git"),
            })
        }
        _ => None,
    }
}

/// Does this CLI argument look like something to clone?
pub fn is_git_uri(arg: &str) -> bool {
    arg.starts_with("http://")
        || arg.starts_with("https://")
        || arg.starts_with("git@")
        || arg.contains("github.com")
        || arg.contains("gitlab.com")
        || arg.ends_with(".git")
}

/// Directory name for a clone: the custom name verbatim, otherwise
/// `YYYY-MM-DD-user-repo`.
pub fn clone_dir_name(input: &str, custom: Option<&str>, date: &str) -> Option<String> {
    if let Some(c) = custom.filter(|c| !c.is_empty()) {
        return Some(naming::dashify(c));
    }
    let (user, repo) = match github_pr(input) {
        Some(pr) => (pr.user, pr.repo),
        None => {
            let u = parse_git_uri(input)?;
            (u.user, u.repo)
        }
    };
    Some(format!("{date}-{user}-{repo}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> (String, String, String) {
        let u = parse_git_uri(s).unwrap_or_else(|| panic!("no parse: {s}"));
        (u.host, u.user, u.repo)
    }

    fn t(h: &str, u: &str, r: &str) -> (String, String, String) {
        (h.into(), u.into(), r.into())
    }

    #[test]
    fn parses_supported_shapes() {
        assert_eq!(
            p("https://github.com/tobi/try.git"),
            t("github.com", "tobi", "try")
        );
        assert_eq!(
            p("https://github.com/tobi/try"),
            t("github.com", "tobi", "try")
        );
        assert_eq!(
            p("git@github.com:tobi/try.git"),
            t("github.com", "tobi", "try")
        );
        assert_eq!(p("https://gitlab.com/u/r.git"), t("gitlab.com", "u", "r"));
        assert_eq!(
            p("git@host.com:user/repo.git"),
            t("host.com", "user", "repo")
        );
        assert_eq!(
            p("ssh://git@host.com:2222/user/repo.git"),
            t("host.com:2222", "user", "repo")
        );
        assert_eq!(
            p("deploy@git.example.com:src/team/project.git"),
            t("git.example.com", "deploy", "project")
        );
        assert_eq!(p("git@host:team/a/b/c.git"), t("host", "team", "c"));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_git_uri("not a url"), None);
        assert_eq!(parse_git_uri("https://github.com/only-user"), None);
        assert_eq!(parse_git_uri(""), None);
    }

    #[test]
    fn pull_requests() {
        let pr = github_pr("https://github.com/tobi/try/pull/124").unwrap();
        assert_eq!(pr.number, "124");
        assert_eq!(pr.clone_url, "https://github.com/tobi/try.git");
        assert!(github_pr("https://www.github.com/a/b/pull/1/").is_some());
        assert!(github_pr("https://github.com/a/b/pull/x").is_none());
        assert!(github_pr("https://github.com/a/b/pull/1/files").is_none());
        assert!(github_pr("https://github.com/a/b").is_none());
    }

    #[test]
    fn clone_names() {
        let d = "2025-11-30";
        assert_eq!(
            clone_dir_name("https://github.com/u/r.git", None, d).unwrap(),
            "2025-11-30-u-r"
        );
        assert_eq!(
            clone_dir_name("https://github.com/u/r/pull/9", None, d).unwrap(),
            "2025-11-30-u-r"
        );
        assert_eq!(
            clone_dir_name("https://github.com/u/r", Some("my fork"), d).unwrap(),
            "my-fork"
        );
        assert_eq!(clone_dir_name("nonsense", None, d), None);
    }

    #[test]
    fn git_uri_heuristic() {
        assert!(is_git_uri("https://example.com/x"));
        assert!(is_git_uri("git@host:a/b"));
        assert!(is_git_uri("github.com/a/b"));
        assert!(is_git_uri("thing.git"));
        assert!(!is_git_uri("redis"));
    }
}
