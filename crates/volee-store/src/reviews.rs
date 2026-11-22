//! The AI pre-reviews of a branch tip (v2): one row per `(repo, branch, tip)`,
//! so that a tip is never reviewed twice, and its findings.
//!
//! A review is an opinion: nothing here gates anything.

use rusqlite::{OptionalExtension, params};

use crate::error::StoreError;
use crate::store::Store;

/// Where a review is. A review starts `pending`, becomes `running` when a
/// slot frees, then `done` or `failed`.
pub mod status {
    /// Waiting for a slot.
    pub const PENDING: &str = "pending";
    /// The reviewer is running.
    pub const RUNNING: &str = "running";
    /// Finished, findings stored (possibly none).
    pub const DONE: &str = "done";
    /// The reviewer failed, timed out or was interrupted.
    pub const FAILED: &str = "failed";
}

/// A finding to store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewFinding {
    /// The file, relative to the repository when it could be made so.
    pub file: String,
    /// The line, when the reviewer gave one.
    pub line: Option<u32>,
    /// `important`, `nit` or `pre_existing`.
    pub severity: String,
    /// What is wrong.
    pub summary: String,
    /// The input and the wrong result, when the reviewer gave them.
    pub failure_scenario: Option<String>,
}

/// A stored review.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredReview {
    /// Its row id.
    pub id: i64,
    /// The repository's main checkout, as the daemon names it.
    pub repo: String,
    /// The reviewed branch.
    pub branch: String,
    /// The reviewed commit.
    pub tip: String,
    /// The base the change was read against.
    pub base: String,
    /// One of [`status`].
    pub status: String,
    /// How many reviewers ran or will run.
    pub reviewers: u32,
    /// The model name given to the reviewer.
    pub model: String,
    /// When it was requested, in milliseconds since the Unix epoch.
    pub requested_at: i64,
    /// When the reviewer started.
    pub started_at: Option<i64>,
    /// When it finished or failed.
    pub finished_at: Option<i64>,
    /// Why it failed.
    pub error: Option<String>,
    /// What the reviewer said, as text, when it said anything.
    pub raw: Option<String>,
    /// What it cost, when the reviewer reported it.
    pub cost_usd: Option<f64>,
    /// The findings, in the order they were stored.
    pub findings: Vec<NewFinding>,
}

/// A new review to record.
#[derive(Clone, Debug)]
pub struct NewReview<'a> {
    /// The repository.
    pub repo: &'a str,
    /// The branch.
    pub branch: &'a str,
    /// The tip.
    pub tip: &'a str,
    /// The base.
    pub base: &'a str,
    /// How many reviewers.
    pub reviewers: u32,
    /// The model name.
    pub model: &'a str,
    /// When, in milliseconds since the Unix epoch.
    pub at: i64,
}

impl Store {
    /// Records a pending review. `None` when the tip already has one: never
    /// review the same tip twice.
    pub fn begin_review(&self, new: &NewReview<'_>) -> Result<Option<i64>, StoreError> {
        let inserted = self.connection.execute(
            "INSERT OR IGNORE INTO reviews \
             (repo, branch, tip, base, status, reviewers, model, requested_at) \
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?6, ?7)",
            params![
                new.repo,
                new.branch,
                new.tip,
                new.base,
                new.reviewers,
                new.model,
                new.at
            ],
        )?;
        Ok((inserted > 0).then(|| self.connection.last_insert_rowid()))
    }

    /// Puts a `failed` review back to `pending` (an explicit retry). `false`
    /// when the tip has no failed review.
    pub fn retry_review(&self, id: i64, at: i64) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE reviews SET status = 'pending', error = NULL, started_at = NULL, \
             finished_at = NULL, requested_at = ?2 WHERE id = ?1 AND status = 'failed'",
            params![id, at],
        )?;
        Ok(changed > 0)
    }

    /// Marks a review as running.
    pub fn start_review(&self, id: i64, at: i64) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE reviews SET status = 'running', started_at = ?2 WHERE id = ?1",
            params![id, at],
        )?;
        Ok(())
    }

    /// Stores the findings of a review and marks it done.
    pub fn finish_review(
        &mut self,
        id: i64,
        at: i64,
        raw: &str,
        cost_usd: Option<f64>,
        findings: &[NewFinding],
    ) -> Result<(), StoreError> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "DELETE FROM review_findings WHERE review_id = ?1",
            params![id],
        )?;
        for (position, finding) in findings.iter().enumerate() {
            transaction.execute(
                "INSERT INTO review_findings \
                 (review_id, position, file, line, severity, summary, failure_scenario) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    i64::try_from(position).unwrap_or(i64::MAX),
                    finding.file,
                    finding.line,
                    finding.severity,
                    finding.summary,
                    finding.failure_scenario
                ],
            )?;
        }
        transaction.execute(
            "UPDATE reviews SET status = 'done', finished_at = ?2, raw = ?3, cost_usd = ?4, \
             error = NULL WHERE id = ?1",
            params![id, at, raw, cost_usd],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Marks a review as failed, with the reason.
    pub fn fail_review(&self, id: i64, at: i64, error: &str) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE reviews SET status = 'failed', finished_at = ?2, error = ?3 WHERE id = ?1",
            params![id, at, error],
        )?;
        Ok(())
    }

    /// Fails the reviews a stopped daemon left `pending` or `running`.
    /// Returns how many.
    pub fn interrupt_reviews(&self, at: i64) -> Result<usize, StoreError> {
        Ok(self.connection.execute(
            "UPDATE reviews SET status = 'failed', finished_at = ?1, \
             error = 'interrupted: the daemon stopped before the review finished' \
             WHERE status IN ('pending', 'running')",
            params![at],
        )?)
    }

    /// The review of a branch at a tip, with its findings.
    pub fn review(
        &self,
        repo: &str,
        branch: &str,
        tip: &str,
    ) -> Result<Option<StoredReview>, StoreError> {
        let found = self
            .connection
            .query_row(
                "SELECT id, repo, branch, tip, base, status, reviewers, model, requested_at, \
                 started_at, finished_at, error, raw, cost_usd FROM reviews \
                 WHERE repo = ?1 AND branch = ?2 AND tip = ?3",
                params![repo, branch, tip],
                |row| {
                    Ok(StoredReview {
                        id: row.get(0)?,
                        repo: row.get(1)?,
                        branch: row.get(2)?,
                        tip: row.get(3)?,
                        base: row.get(4)?,
                        status: row.get(5)?,
                        reviewers: row.get(6)?,
                        model: row.get(7)?,
                        requested_at: row.get(8)?,
                        started_at: row.get(9)?,
                        finished_at: row.get(10)?,
                        error: row.get(11)?,
                        raw: row.get(12)?,
                        cost_usd: row.get(13)?,
                        findings: Vec::new(),
                    })
                },
            )
            .optional()?;
        let Some(mut review) = found else {
            return Ok(None);
        };
        let mut statement = self.connection.prepare(
            "SELECT file, line, severity, summary, failure_scenario FROM review_findings \
             WHERE review_id = ?1 ORDER BY position",
        )?;
        let rows = statement.query_map(params![review.id], |row| {
            Ok(NewFinding {
                file: row.get(0)?,
                line: row.get(1)?,
                severity: row.get(2)?,
                summary: row.get(3)?,
                failure_scenario: row.get(4)?,
            })
        })?;
        review.findings = rows.collect::<Result<_, _>>()?;
        Ok(Some(review))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new(tip: &str) -> NewReview<'_> {
        NewReview {
            repo: "/r",
            branch: "feat",
            tip,
            base: "main",
            reviewers: 1,
            model: "haiku",
            at: 10,
        }
    }

    fn finding(file: &str) -> NewFinding {
        NewFinding {
            file: file.to_owned(),
            line: Some(3),
            severity: "important".to_owned(),
            summary: "wrong".to_owned(),
            failure_scenario: Some("f(1) crashes".to_owned()),
        }
    }

    #[test]
    fn a_tip_is_recorded_once() {
        let store = Store::open_in_memory().unwrap();
        assert!(store.begin_review(&new("aaa")).unwrap().is_some());
        assert!(store.begin_review(&new("aaa")).unwrap().is_none());
        assert!(store.begin_review(&new("bbb")).unwrap().is_some());
    }

    #[test]
    fn a_review_goes_from_pending_to_done_with_its_findings() {
        let mut store = Store::open_in_memory().unwrap();
        let id = store.begin_review(&new("aaa")).unwrap().unwrap();
        let pending = store.review("/r", "feat", "aaa").unwrap().unwrap();
        assert_eq!(pending.status, status::PENDING);
        store.start_review(id, 11).unwrap();
        store
            .finish_review(
                id,
                12,
                "text",
                Some(0.01),
                &[finding("a.py"), finding("b.py")],
            )
            .unwrap();
        let done = store.review("/r", "feat", "aaa").unwrap().unwrap();
        assert_eq!(done.status, status::DONE);
        assert_eq!(done.started_at, Some(11));
        assert_eq!(done.finished_at, Some(12));
        assert_eq!(done.cost_usd, Some(0.01));
        assert_eq!(done.findings.len(), 2);
        assert_eq!(done.findings[1].file, "b.py");
        assert!(store.review("/r", "feat", "zzz").unwrap().is_none());
    }

    #[test]
    fn a_failed_review_can_be_retried_but_a_done_one_cannot() {
        let mut store = Store::open_in_memory().unwrap();
        let id = store.begin_review(&new("aaa")).unwrap().unwrap();
        store.fail_review(id, 12, "timed out").unwrap();
        let failed = store.review("/r", "feat", "aaa").unwrap().unwrap();
        assert_eq!(failed.error.as_deref(), Some("timed out"));
        assert!(store.retry_review(id, 20).unwrap());
        assert_eq!(
            store.review("/r", "feat", "aaa").unwrap().unwrap().status,
            status::PENDING
        );
        store.finish_review(id, 21, "", None, &[]).unwrap();
        assert!(!store.retry_review(id, 22).unwrap());
    }

    #[test]
    fn a_stopped_daemon_leaves_nothing_running() {
        let store = Store::open_in_memory().unwrap();
        let a = store.begin_review(&new("aaa")).unwrap().unwrap();
        store.begin_review(&new("bbb")).unwrap();
        store.start_review(a, 11).unwrap();
        assert_eq!(store.interrupt_reviews(30).unwrap(), 2);
        let review = store.review("/r", "feat", "aaa").unwrap().unwrap();
        assert_eq!(review.status, status::FAILED);
        assert!(review.error.unwrap().contains("interrupted"));
    }
}
