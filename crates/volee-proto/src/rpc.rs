//! JSON-RPC 2.0 envelopes, one JSON object per line.

use std::fmt;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

/// The `"jsonrpc": "2.0"` marker. Deserializing anything else fails.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Version2;

impl Serialize for Version2 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("2.0")
    }
}

impl<'de> Deserialize<'de> for Version2 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Version2;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("the string \"2.0\"")
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Version2, E> {
                if value == "2.0" {
                    Ok(Version2)
                } else {
                    Err(E::invalid_value(de::Unexpected::Str(value), &self))
                }
            }
        }
        deserializer.deserialize_str(V)
    }
}

/// A request identifier.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    /// A number, as volee clients send.
    Number(u64),
    /// A string, as some other clients send.
    String(String),
}

/// A request, or a notification when `id` is absent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Always `"2.0"`.
    pub jsonrpc: Version2,
    /// Present for requests that expect a response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    /// The method name, such as `daemon.status`.
    pub method: String,
    /// The parameters; `null` when the method takes none.
    #[serde(default)]
    pub params: Value,
}

/// A response to a request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// Always `"2.0"`.
    pub jsonrpc: Version2,
    /// The identifier of the request this answers; `null` when the request
    /// could not be read.
    pub id: Option<Id>,
    /// The result or the error.
    #[serde(flatten)]
    pub outcome: Outcome,
}

/// The result or the error of a request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Outcome {
    /// The method succeeded.
    #[serde(rename = "result")]
    Result(Value),
    /// The method failed.
    #[serde(rename = "error")]
    Error(RpcError),
}

/// A message sent by the daemon without a request, such as a new event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    /// Always `"2.0"`.
    pub jsonrpc: Version2,
    /// What happened, such as `events.appended`.
    pub method: String,
    /// The payload.
    #[serde(default)]
    pub params: Value,
}

/// Anything a client can receive.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Incoming {
    /// A response to one of its requests.
    Response(Response),
    /// A notification.
    Notification(Notification),
}

/// An error, as defined by JSON-RPC 2.0, with volee's own codes for domain
/// errors.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message} (code {code})")]
pub struct RpcError {
    /// The error code; see the constants in this module.
    pub code: i64,
    /// A sentence for people.
    pub message: String,
    /// Structured details, when useful.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// The line was not valid JSON.
pub const PARSE_ERROR: i64 = -32_700;
/// The JSON was not a valid request.
pub const INVALID_REQUEST: i64 = -32_600;
/// The method does not exist.
pub const METHOD_NOT_FOUND: i64 = -32_601;
/// The parameters do not match the method.
pub const INVALID_PARAMS: i64 = -32_602;
/// The daemon failed unexpectedly.
pub const INTERNAL_ERROR: i64 = -32_603;
/// The named agent, message, thread or mission does not exist.
pub const NOT_FOUND: i64 = -32_001;
/// The request is valid but refused by a rule, for example a routing or a
/// cleanup rule.
pub const REFUSED: i64 = -32_002;
/// A required external program (a harness, git) is not available.
pub const UNAVAILABLE: i64 = -32_003;
/// The client and the daemon speak different protocol versions.
pub const VERSION_MISMATCH: i64 = -32_004;

impl RpcError {
    /// An error with a code and a message.
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_request_round_trips() {
        let line = r#"{"jsonrpc":"2.0","id":7,"method":"daemon.status","params":null}"#;
        let request: Request = serde_json::from_str(line).unwrap();
        assert_eq!(request.id, Some(Id::Number(7)));
        assert_eq!(request.method, "daemon.status");
        assert_eq!(serde_json::to_string(&request).unwrap(), line);
    }

    #[test]
    fn a_wrong_version_is_rejected() {
        let line = r#"{"jsonrpc":"1.0","id":1,"method":"x"}"#;
        assert!(serde_json::from_str::<Request>(line).is_err());
    }

    #[test]
    fn responses_carry_either_a_result_or_an_error() {
        let ok = Response {
            jsonrpc: Version2,
            id: Some(Id::Number(1)),
            outcome: Outcome::Result(json!({"pid": 42})),
        };
        assert_eq!(
            serde_json::to_value(&ok).unwrap(),
            json!({"jsonrpc":"2.0","id":1,"result":{"pid":42}})
        );
        let failed = Response {
            jsonrpc: Version2,
            id: Some(Id::String("a".into())),
            outcome: Outcome::Error(RpcError::new(NOT_FOUND, "unknown agent")),
        };
        assert_eq!(
            serde_json::to_value(&failed).unwrap(),
            json!({"jsonrpc":"2.0","id":"a","error":{"code":-32001,"message":"unknown agent"}})
        );
    }

    #[test]
    fn an_unreadable_request_is_answered_with_a_null_id() {
        let response = Response {
            jsonrpc: Version2,
            id: None,
            outcome: Outcome::Error(RpcError::new(PARSE_ERROR, "not JSON")),
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["id"], Value::Null);
    }

    #[test]
    fn incoming_tells_responses_from_notifications() {
        let response: Incoming =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":3,"result":true}"#).unwrap();
        assert!(matches!(response, Incoming::Response(_)));
        let notification: Incoming =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"events.appended","params":{}}"#)
                .unwrap();
        assert!(matches!(notification, Incoming::Notification(_)));
    }
}
