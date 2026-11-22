//! What the daemon remembers about branches between two readings and across
//! restarts (v2): the last tip seen with commits of its own, which proves a
//! merge once that commit is part of the base, and the answers of the forge
//! about a branch's pull request, which cost a network call.

use rusqlite::{OptionalExtension, params};

use crate::error::StoreError;
use crate::store::Store;

/// A pull request state the forge answered for a branch at a tip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedPullRequest {
    /// `merged`, `open`, `draft`, `closed`, `none` (no pull request) or
    /// `error` (the forge could not be asked).
    pub state: String,
    /// The pull request's number, when there is one.
    pub number: Option<i64>,
    /// Its web address, when there is one.
    pub url: Option<String>,
    /// When the forge answered, in milliseconds since the Unix epoch.
    pub checked_at: i64,
}

impl Store {
    /// The last tip seen for `branch` of `repo` with commits its base did
    /// not have.
    pub fn unmerged_tip(&self, repo: &str, branch: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .connection
            .query_row(
                "SELECT unmerged_tip FROM branch_memory WHERE repo = ?1 AND branch = ?2",
                params![repo, branch],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Remembers the tip, or with `None` forgets it.
    pub fn set_unmerged_tip(
        &self,
        repo: &str,
        branch: &str,
        tip: Option<&str>,
        at: i64,
    ) -> Result<(), StoreError> {
        match tip {
            Some(tip) => {
                self.connection.execute(
                    "INSERT INTO branch_memory (repo, branch, unmerged_tip, updated_at) \
                     VALUES (?1, ?2, ?3, ?4) \
                     ON CONFLICT (repo, branch) DO UPDATE SET \
                         unmerged_tip = excluded.unmerged_tip, updated_at = excluded.updated_at",
                    params![repo, branch, tip, at],
                )?;
            }
            None => {
                self.connection.execute(
                    "DELETE FROM branch_memory WHERE repo = ?1 AND branch = ?2",
                    params![repo, branch],
                )?;
            }
        }
        Ok(())
    }

    /// The forge's last answer about the pull request of `branch` at `tip`.
    pub fn cached_pull_request(
        &self,
        repo: &str,
        branch: &str,
        tip: &str,
    ) -> Result<Option<CachedPullRequest>, StoreError> {
        Ok(self
            .connection
            .query_row(
                "SELECT state, number, url, checked_at FROM pull_request_cache \
                 WHERE repo = ?1 AND branch = ?2 AND tip = ?3",
                params![repo, branch, tip],
                |row| {
                    Ok(CachedPullRequest {
                        state: row.get(0)?,
                        number: row.get(1)?,
                        url: row.get(2)?,
                        checked_at: row.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    /// Stores or replaces the forge's answer about `branch` at `tip`.
    pub fn cache_pull_request(
        &self,
        repo: &str,
        branch: &str,
        tip: &str,
        answer: &CachedPullRequest,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO pull_request_cache (repo, branch, tip, state, number, url, checked_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT (repo, branch, tip) DO UPDATE SET \
                 state = excluded.state, number = excluded.number, url = excluded.url, \
                 checked_at = excluded.checked_at",
            params![
                repo,
                branch,
                tip,
                answer.state,
                answer.number,
                answer.url,
                answer.checked_at
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unmerged_tip_is_remembered_replaced_and_forgotten() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.unmerged_tip("/r", "feat").unwrap(), None);
        store
            .set_unmerged_tip("/r", "feat", Some("aaa"), 1)
            .unwrap();
        store
            .set_unmerged_tip("/r", "other", Some("bbb"), 1)
            .unwrap();
        store
            .set_unmerged_tip("/r", "feat", Some("ccc"), 2)
            .unwrap();
        assert_eq!(
            store.unmerged_tip("/r", "feat").unwrap().as_deref(),
            Some("ccc")
        );
        assert_eq!(
            store.unmerged_tip("/r", "other").unwrap().as_deref(),
            Some("bbb")
        );
        assert_eq!(store.unmerged_tip("/elsewhere", "feat").unwrap(), None);
        store.set_unmerged_tip("/r", "feat", None, 3).unwrap();
        assert_eq!(store.unmerged_tip("/r", "feat").unwrap(), None);
        assert_eq!(
            store.unmerged_tip("/r", "other").unwrap().as_deref(),
            Some("bbb")
        );
    }

    #[test]
    fn a_pull_request_answer_belongs_to_one_branch_at_one_tip() {
        let store = Store::open_in_memory().unwrap();
        let merged = CachedPullRequest {
            state: "merged".into(),
            number: Some(7),
            url: Some("https://example.com/pull/7".into()),
            checked_at: 10,
        };
        store
            .cache_pull_request("/r", "feat", "t1", &merged)
            .unwrap();
        assert_eq!(
            store.cached_pull_request("/r", "feat", "t1").unwrap(),
            Some(merged.clone())
        );
        assert_eq!(store.cached_pull_request("/r", "feat", "t2").unwrap(), None);
        let none = CachedPullRequest {
            state: "none".into(),
            number: None,
            url: None,
            checked_at: 20,
        };
        store.cache_pull_request("/r", "feat", "t1", &none).unwrap();
        assert_eq!(
            store.cached_pull_request("/r", "feat", "t1").unwrap(),
            Some(none)
        );
    }
}
