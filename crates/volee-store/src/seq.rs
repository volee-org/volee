//! Sequence numbers of the event log.

use std::fmt;

use rusqlite::ToSql;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef};

/// The position of an event in the log. Assigned by the store, strictly
/// increasing, starting at 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seq(u64);

impl Seq {
    /// Wraps a raw sequence number, for example one received from a client.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The raw sequence number.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Seq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

impl ToSql for Seq {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        i64::try_from(self.0)
            .map(ToSqlOutput::from)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
    }
}

impl FromSql for Seq {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let raw = i64::column_result(value)?;
        u64::try_from(raw)
            .map(Self)
            .map_err(|_| FromSqlError::OutOfRange(raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_with_a_hash() {
        assert_eq!(Seq::new(7).to_string(), "#7");
        assert_eq!(Seq::new(7).get(), 7);
    }

    #[test]
    fn values_beyond_i64_are_refused_by_sql() {
        assert!(Seq::new(u64::MAX).to_sql().is_err());
        assert!(Seq::new(5).to_sql().is_ok());
    }
}
