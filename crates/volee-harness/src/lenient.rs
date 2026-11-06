//! Tolerant deserialization of harness output.
//!
//! Harnesses add fields and occasionally change their types. An optional
//! field with an unexpected type becomes `None` instead of rejecting the
//! whole object: losing one detail is better than losing a hook event or a
//! session.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// Deserializes an optional field, turning a value of an unexpected type into
/// `None`. Use with `#[serde(default, deserialize_with = "lenient::optional")]`.
pub(crate) fn optional<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = Value::deserialize(deserializer)?;
    Ok(T::deserialize(value).ok())
}

/// Deserializes a model name given either as a string or as an object with an
/// `id` (or a `display_name`).
pub(crate) fn model<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Value::deserialize(deserializer)? {
        Value::String(name) => Some(name),
        Value::Object(fields) => fields
            .get("id")
            .or_else(|| fields.get("display_name"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize)]
    struct Sample {
        #[serde(default, deserialize_with = "optional")]
        name: Option<String>,
        #[serde(default, deserialize_with = "optional")]
        count: Option<u32>,
        #[serde(default, deserialize_with = "model")]
        model: Option<String>,
    }

    #[test]
    fn wrong_types_become_none() {
        let sample: Sample =
            serde_json::from_str(r#"{"name": 42, "count": "seven", "model": [1]}"#).unwrap();
        assert_eq!(sample.name, None);
        assert_eq!(sample.count, None);
        assert_eq!(sample.model, None);
    }

    #[test]
    fn right_types_and_missing_fields() {
        let sample: Sample = serde_json::from_str(r#"{"name": "x", "count": 7}"#).unwrap();
        assert_eq!(sample.name.as_deref(), Some("x"));
        assert_eq!(sample.count, Some(7));
        assert_eq!(sample.model, None);
    }

    #[test]
    fn models_come_as_strings_or_objects() {
        let text: Sample = serde_json::from_str(r#"{"model": "claude-opus-5"}"#).unwrap();
        assert_eq!(text.model.as_deref(), Some("claude-opus-5"));
        let object: Sample =
            serde_json::from_str(r#"{"model": {"id": "claude-opus-5", "display_name": "Opus"}}"#)
                .unwrap();
        assert_eq!(object.model.as_deref(), Some("claude-opus-5"));
    }
}
