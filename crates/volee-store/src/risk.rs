//! The risk a person set by hand on a branch tip (v2, step 8).

use rusqlite::{OptionalExtension, params};

use crate::error::StoreError;
use crate::store::Store;

impl Store {
    /// The level (`low`, `medium` or `high`) the person set on `tip` of the
    /// repository at `repo`, if they did.
    pub fn risk_override(&self, repo: &str, tip: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .connection
            .query_row(
                "SELECT level FROM risk_overrides WHERE repo = ?1 AND tip = ?2",
                params![repo, tip],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Sets the level the person chose for `tip`, or with `None` takes the
    /// choice back.
    pub fn set_risk_override(
        &self,
        repo: &str,
        tip: &str,
        level: Option<&str>,
        at: i64,
    ) -> Result<(), StoreError> {
        match level {
            Some(level) => {
                self.connection.execute(
                    "INSERT INTO risk_overrides (repo, tip, level, set_at) VALUES (?1, ?2, ?3, ?4) \
                     ON CONFLICT (repo, tip) DO UPDATE SET \
                         level = excluded.level, set_at = excluded.set_at",
                    params![repo, tip, level, at],
                )?;
            }
            None => {
                self.connection.execute(
                    "DELETE FROM risk_overrides WHERE repo = ?1 AND tip = ?2",
                    params![repo, tip],
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_override_is_per_tip_and_can_be_taken_back() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.risk_override("/r", "abc").unwrap(), None);
        store
            .set_risk_override("/r", "abc", Some("low"), 1)
            .unwrap();
        assert_eq!(
            store.risk_override("/r", "abc").unwrap().as_deref(),
            Some("low")
        );
        assert_eq!(
            store.risk_override("/r", "def").unwrap(),
            None,
            "a new tip is judged again"
        );
        assert_eq!(store.risk_override("/other", "abc").unwrap(), None);
        store
            .set_risk_override("/r", "abc", Some("high"), 2)
            .unwrap();
        assert_eq!(
            store.risk_override("/r", "abc").unwrap().as_deref(),
            Some("high")
        );
        store.set_risk_override("/r", "abc", None, 3).unwrap();
        assert_eq!(store.risk_override("/r", "abc").unwrap(), None);
    }
}
