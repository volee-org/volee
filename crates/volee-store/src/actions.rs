//! What the person does on a finished change (v2, step 9): the comments on
//! lines of its diff, kept per branch tip, and the journal of actions.

use rusqlite::params;
use serde::Serialize;

use crate::error::StoreError;
use crate::store::Store;

/// A comment on a line of a change, as stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StoredComment {
    /// Its id.
    pub id: i64,
    /// The file, relative to the repository.
    pub file: String,
    /// The line, on the new side of the diff.
    pub line: u32,
    /// What the person wrote.
    pub text: String,
    /// When, in milliseconds.
    pub created_at: i64,
}

/// An action to write in the journal.
#[derive(Clone, Copy, Debug)]
pub struct NewJournalEntry<'a> {
    /// When, in milliseconds.
    pub at: i64,
    /// The repository's main checkout.
    pub repo: &'a str,
    /// The branch the action is about.
    pub branch: &'a str,
    /// Its tip then.
    pub tip: &'a str,
    /// `merge`, `fix`, `comment` or `editor`.
    pub kind: &'a str,
    /// Whether it worked.
    pub ok: bool,
    /// Volee did it by itself, by a rule, rather than on a click.
    pub automatic: bool,
    /// What happened, in a sentence.
    pub message: &'a str,
}

/// An entry of the journal, as read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct JournalEntry {
    /// Its id, growing.
    pub id: i64,
    /// When, in milliseconds.
    pub at: i64,
    /// The repository's main checkout.
    pub repo: String,
    /// The branch.
    pub branch: String,
    /// Its tip then.
    pub tip: String,
    /// `merge`, `fix`, `comment` or `editor`.
    pub kind: String,
    /// Whether it worked.
    pub ok: bool,
    /// Volee did it by itself.
    pub automatic: bool,
    /// What happened.
    pub message: String,
}

impl Store {
    /// Adds a comment on `file` at `line` of the change at `tip`.
    pub fn add_comment(
        &self,
        repo: &str,
        tip: &str,
        file: &str,
        line: u32,
        text: &str,
        at: i64,
    ) -> Result<i64, StoreError> {
        self.connection.execute(
            "INSERT INTO line_comments (repo, tip, file, line, text, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![repo, tip, file, line, text, at],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    /// The comments on the change at `tip`, in the order they were written.
    pub fn comments(&self, repo: &str, tip: &str) -> Result<Vec<StoredComment>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, file, line, text, created_at FROM line_comments \
             WHERE repo = ?1 AND tip = ?2 ORDER BY id",
        )?;
        let rows = statement.query_map(params![repo, tip], |row| {
            Ok(StoredComment {
                id: row.get(0)?,
                file: row.get(1)?,
                line: row.get(2)?,
                text: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Removes a comment of the change at `tip`; whether there was one.
    pub fn delete_comment(&self, repo: &str, tip: &str, id: i64) -> Result<bool, StoreError> {
        let removed = self.connection.execute(
            "DELETE FROM line_comments WHERE id = ?1 AND repo = ?2 AND tip = ?3",
            params![id, repo, tip],
        )?;
        Ok(removed > 0)
    }

    /// Writes an action in the journal.
    pub fn journal_add(&self, entry: &NewJournalEntry<'_>) -> Result<i64, StoreError> {
        self.connection.execute(
            "INSERT INTO journal (at, repo, branch, tip, kind, ok, automatic, message) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                entry.at,
                entry.repo,
                entry.branch,
                entry.tip,
                entry.kind,
                entry.ok,
                entry.automatic,
                entry.message
            ],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    /// The latest `limit` entries of the journal, newest first.
    pub fn journal(&self, limit: u32) -> Result<Vec<JournalEntry>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, at, repo, branch, tip, kind, ok, automatic, message \
             FROM journal ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = statement.query_map(params![limit], |row| {
            Ok(JournalEntry {
                id: row.get(0)?,
                at: row.get(1)?,
                repo: row.get(2)?,
                branch: row.get(3)?,
                tip: row.get(4)?,
                kind: row.get(5)?,
                ok: row.get(6)?,
                automatic: row.get(7)?,
                message: row.get(8)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Whether the journal has an action of `kind` for the change at `tip`
    /// (any outcome): what keeps an automatic action from repeating.
    pub fn journal_has(&self, repo: &str, tip: &str, kind: &str) -> Result<bool, StoreError> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM journal WHERE repo = ?1 AND tip = ?2 AND kind = ?3",
            params![repo, tip, kind],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_belong_to_a_tip_and_can_be_removed() {
        let store = Store::open_in_memory().unwrap();
        let id = store
            .add_comment("/r", "abc", "a.rs", 3, "why?", 1)
            .unwrap();
        store
            .add_comment("/r", "abc", "b.rs", 9, "and this", 2)
            .unwrap();
        store
            .add_comment("/r", "def", "a.rs", 3, "other tip", 3)
            .unwrap();
        let here = store.comments("/r", "abc").unwrap();
        assert_eq!(here.len(), 2);
        assert_eq!(here[0].text, "why?");
        assert_eq!(here[1].line, 9);
        assert!(!store.delete_comment("/r", "def", id).unwrap(), "wrong tip");
        assert!(store.delete_comment("/r", "abc", id).unwrap());
        assert_eq!(store.comments("/r", "abc").unwrap().len(), 1);
    }

    #[test]
    fn the_journal_lists_newest_first_and_remembers_what_was_tried() {
        let store = Store::open_in_memory().unwrap();
        let entry = |kind, at, ok| NewJournalEntry {
            at,
            repo: "/r",
            branch: "feat",
            tip: "abc",
            kind,
            ok,
            automatic: kind == "merge",
            message: "m",
        };
        store.journal_add(&entry("fix", 1, true)).unwrap();
        store.journal_add(&entry("merge", 2, false)).unwrap();
        let all = store.journal(10).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].kind, "merge");
        assert!(!all[0].ok && all[0].automatic);
        assert_eq!(store.journal(1).unwrap().len(), 1);
        assert!(store.journal_has("/r", "abc", "merge").unwrap());
        assert!(!store.journal_has("/r", "def", "merge").unwrap());
    }
}
