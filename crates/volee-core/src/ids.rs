//! Identifiers of volee's own objects.
//!
//! Every identifier is a [ULID](https://github.com/ulid/spec) behind a short
//! type prefix, rendered in lowercase: `ag_01jc5…` for an agent. ULIDs sort by
//! creation time, which keeps lists and the event log in a natural order.
//!
//! Identifiers are created by the daemon; this crate only parses, formats and
//! compares them.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// The error returned when a string is not a valid identifier.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{input}` is not a valid {kind} identifier (expected `{prefix}_` followed by a ULID)")]
pub struct ParseIdError {
    input: String,
    kind: &'static str,
    prefix: &'static str,
}

macro_rules! prefixed_id {
    ($(#[$meta:meta])* $name:ident, $prefix:literal, $kind:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(Ulid);

        impl $name {
            /// The prefix written before the ULID.
            pub const PREFIX: &'static str = $prefix;

            /// Wraps an existing ULID.
            pub const fn from_ulid(ulid: Ulid) -> Self {
                Self(ulid)
            }

            /// The underlying ULID.
            pub const fn ulid(self) -> Ulid {
                self.0
            }

            /// A short form for display: the prefix and the last six
            /// characters, which come from the random part of the ULID.
            pub fn short(self) -> String {
                let full = self.0.to_string().to_lowercase();
                format!("{}_{}", Self::PREFIX, &full[full.len() - 6..])
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}_{}", Self::PREFIX, self.0.to_string().to_lowercase())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(self, f)
            }
        }

        impl FromStr for $name {
            type Err = ParseIdError;

            fn from_str(input: &str) -> Result<Self, Self::Err> {
                let error = || ParseIdError {
                    input: input.to_owned(),
                    kind: $kind,
                    prefix: $prefix,
                };
                let rest = input
                    .strip_prefix($prefix)
                    .and_then(|rest| rest.strip_prefix('_'))
                    .ok_or_else(error)?;
                Ulid::from_string(&rest.to_uppercase())
                    .map(Self)
                    .map_err(|_| error())
            }
        }

        impl TryFrom<String> for $name {
            type Error = ParseIdError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                value.parse()
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> Self {
                id.to_string()
            }
        }
    };
}

prefixed_id!(
    /// Identifies an agent across all of its harness sessions.
    AgentId,
    "ag",
    "agent"
);
prefixed_id!(
    /// Identifies a mission.
    MissionId,
    "ms",
    "mission"
);
prefixed_id!(
    /// Identifies a message.
    MessageId,
    "msg",
    "message"
);
prefixed_id!(
    /// Identifies a conversation thread.
    ThreadId,
    "th",
    "thread"
);

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AgentId {
        AgentId::from_ulid(Ulid::from_parts(1_793_986_617_000, 0x0123_4567_89ab_cdef))
    }

    #[test]
    fn display_and_parse_round_trip() {
        let id = sample();
        let text = id.to_string();
        assert!(text.starts_with("ag_"));
        assert_eq!(text, text.to_lowercase());
        assert_eq!(text.parse::<AgentId>().unwrap(), id);
    }

    #[test]
    fn parsing_accepts_uppercase_ulids() {
        let id = sample();
        let upper = format!("ag_{}", id.ulid());
        assert_eq!(upper.parse::<AgentId>().unwrap(), id);
    }

    #[test]
    fn parsing_rejects_the_wrong_prefix() {
        let text = sample().to_string().replacen("ag_", "ms_", 1);
        let error = text.parse::<AgentId>().unwrap_err();
        assert!(error.to_string().contains("agent"));
        assert!(text.parse::<MissionId>().is_ok());
    }

    #[test]
    fn parsing_rejects_garbage() {
        assert!("ag_".parse::<AgentId>().is_err());
        assert!("ag_not-a-ulid".parse::<AgentId>().is_err());
        assert!("agent".parse::<AgentId>().is_err());
    }

    #[test]
    fn short_form_keeps_the_prefix_and_six_characters() {
        let short = sample().short();
        assert!(short.starts_with("ag_"));
        assert_eq!(short.len(), "ag_".len() + 6);
    }

    #[test]
    fn serde_uses_the_display_form() {
        let id = sample();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{id}\""));
        assert_eq!(serde_json::from_str::<AgentId>(&json).unwrap(), id);
        assert!(serde_json::from_str::<AgentId>("\"nope\"").is_err());
    }

    #[test]
    fn ids_sort_by_creation_time() {
        let older = AgentId::from_ulid(Ulid::from_parts(1_000, 9));
        let newer = AgentId::from_ulid(Ulid::from_parts(2_000, 1));
        assert!(older < newer);
    }
}
