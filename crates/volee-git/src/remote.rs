//! Repositories on a forge, named `owner/name`, as remote URLs give them.

use std::fmt;

/// A repository on a forge, such as `volee-org/volee` on GitHub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoSlug {
    /// The account or organization that owns it.
    pub owner: String,
    /// Its name, without a `.git` suffix.
    pub name: String,
}

impl RepoSlug {
    /// Reads `owner/name` from a remote URL, in any form git accepts:
    /// `git@github.com:owner/name.git`, `https://github.com/owner/name`,
    /// `ssh://git@github.com:22/owner/name.git`... The host is ignored, so
    /// SSH host aliases and GitHub Enterprise hosts work too. `None` when
    /// the URL has no `owner/name` at its end.
    pub fn from_remote_url(url: &str) -> Option<Self> {
        let url = url.trim();
        let path = match url.split_once("://") {
            // A repository on this machine has no owner on a forge.
            Some((scheme, _)) if scheme.eq_ignore_ascii_case("file") => return None,
            // `scheme://[user@]host[:port]/path`
            Some((_, rest)) => rest.split_once('/')?.1,
            // `[user@]host:path`, git's short SSH form. A local path (with a
            // slash before any colon) has no owner on a forge.
            None => {
                let (host, path) = url.split_once(':')?;
                if host.contains('/') {
                    return None;
                }
                path
            }
        };
        let path = path.trim_end_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        let mut parts = path.rsplit('/');
        let name = parts.next().filter(|part| !part.is_empty())?;
        let owner = parts.next().filter(|part| !part.is_empty())?;
        Some(Self {
            owner: owner.to_owned(),
            name: name.to_owned(),
        })
    }

    /// Whether `owner/name` names this repository. Forges such as GitHub
    /// compare both ignoring ASCII case.
    pub fn is(&self, owner: &str, name: &str) -> bool {
        self.owner.eq_ignore_ascii_case(owner) && self.name.eq_ignore_ascii_case(name)
    }
}

impl fmt::Display for RepoSlug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slug(url: &str) -> Option<String> {
        RepoSlug::from_remote_url(url).map(|slug| slug.to_string())
    }

    #[test]
    fn every_url_form_gives_owner_and_name() {
        for url in [
            "git@github.com:volee-org/volee.git",
            "git@github.com:volee-org/volee",
            "github-work:volee-org/volee.git",
            "https://github.com/volee-org/volee.git",
            "https://github.com/volee-org/volee",
            "https://github.com/volee-org/volee/",
            "https://jeff@github.com/volee-org/volee.git",
            "ssh://git@github.com/volee-org/volee.git",
            "ssh://git@github.com:22/volee-org/volee.git",
            "git://github.example.com/volee-org/volee.git",
            " https://github.com/volee-org/volee.git\n",
        ] {
            assert_eq!(slug(url).as_deref(), Some("volee-org/volee"), "{url}");
        }
    }

    #[test]
    fn urls_without_an_owner_give_nothing() {
        for url in [
            "",
            "volee",
            "/srv/git/volee.git",
            "/srv/git/team/volee.git",
            "file:///srv/git/team/volee.git",
            "./volee",
            "https://github.com/volee",
            "git@github.com:volee.git",
        ] {
            assert_eq!(slug(url), None, "{url}");
        }
    }

    #[test]
    fn owners_and_names_compare_without_case() {
        let slug = RepoSlug::from_remote_url("git@github.com:Volee-Org/Volee.git").unwrap();
        assert!(slug.is("volee-org", "volee"));
        assert!(!slug.is("someone", "volee"));
        assert!(!slug.is("volee-org", "volee-fork"));
    }
}
