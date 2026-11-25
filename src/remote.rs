//! Which project of which server a folder is, as its `origin` remote says.

use std::path::Path;
use std::process::Command;

/// A project on a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// The server, with its port if the address gives one for the web
    /// (`gitlab.com`, `git.example.com:8443`).
    pub host: String,
    /// `group/sub/name`.
    pub path: String,
}

/// Reads a remote address: `https://host/group/name.git`,
/// `ssh://git@host:22/group/name.git` or `git@host:group/name.git`.
pub fn parse(url: &str) -> Option<Project> {
    let url = url.trim();
    let (host, path) = if let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    {
        let (authority, path) = rest.split_once('/')?;
        // A user or a token written in the address is not part of the host.
        let host = authority.rsplit('@').next()?;
        (host.to_string(), path)
    } else if let Some(rest) = url.strip_prefix("ssh://") {
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        // The port of ssh is not the one of the web.
        (host.split(':').next()?.to_string(), path)
    } else {
        let (authority, path) = url.split_once(':')?;
        if authority.contains('/') || !authority.contains('@') {
            return None;
        }
        (authority.rsplit('@').next()?.to_string(), path)
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let good = |part: &str| !part.is_empty() && part != "." && part != "..";
    if host.is_empty() || !path.contains('/') || !path.split('/').all(good) {
        return None;
    }
    Some(Project {
        host: host.to_lowercase(),
        path: path.to_string(),
    })
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !text.is_empty()).then_some(text)
}

/// What a folder of the disk says of itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    pub project: Project,
    /// The branch checked out, if one is.
    pub branch: Option<String>,
}

/// The project of the repository holding `path` (a file or a folder): the one
/// of `origin`, else of the first remote.
pub fn detect(path: &Path) -> Option<Checkout> {
    let dir = if path.is_dir() { path } else { path.parent()? };
    let url = git(dir, &["remote", "get-url", "origin"]).or_else(|| {
        let first = git(dir, &["remote"])?.lines().next()?.to_string();
        git(dir, &["remote", "get-url", &first])
    })?;
    Some(Checkout {
        project: parse(&url)?,
        branch: git(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(host: &str, path: &str) -> Option<Project> {
        Some(Project {
            host: host.into(),
            path: path.into(),
        })
    }

    #[test]
    fn every_way_of_writing_a_remote_is_read() {
        assert_eq!(
            parse("git@gitlab.com:group/sub/app.git"),
            project("gitlab.com", "group/sub/app")
        );
        assert_eq!(
            parse("https://oauth2:secret@Git.Example.com:8443/a/b.git/"),
            project("git.example.com:8443", "a/b")
        );
        assert_eq!(
            parse("ssh://git@git.example.com:2222/a/b"),
            project("git.example.com", "a/b")
        );
        for bad in [
            "",
            "/a/local/folder",
            "https://gitlab.com/only-a-name",
            "https://gitlab.com/a/../b",
            "C:\\code\\a",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_folder_says_its_project_and_its_branch() {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            assert!(Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success());
        };
        run(&["init", "-q", "-b", "topic"]);
        assert_eq!(detect(dir.path()), None, "no remote yet");
        run(&["remote", "add", "origin", "git@gitlab.com:me/app.git"]);
        let found = detect(dir.path()).unwrap();
        assert_eq!(found.project.path, "me/app");
        assert_eq!(found.branch.as_deref(), Some("topic"));
    }
}
