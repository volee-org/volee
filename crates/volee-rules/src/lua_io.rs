//! The edges between Lua and Rust: reading and merging the files, turning
//! Lua values into typed answers.

use std::io;
use std::path::Path;

use mlua::chunk::ChunkMode;
use mlua::{Function, Lua, LuaSerdeExt, MultiValue, Table, Value};

use crate::engine::{Evaluated, Source};
use crate::error::RuleError;
use crate::sandbox::Sandbox;
use crate::settings::{FUNCTION_KEYS, SETTING_KEYS, Settings};
use crate::types::{Change, ChangeView, Layer, MergeDecision, ReviewPlan, Risk, RiskAssessment};

pub(crate) fn read_source(path: &Path) -> Result<Option<Source>, RuleError> {
    let name = path.display().to_string();
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(Source { name, text })),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(RuleError {
            file: Some(name),
            line: None,
            function: None,
            message: format!("cannot read the rules file: {e}"),
        }),
    }
}

/// Runs the files in order, merging each returned table over the previous
/// ones, then splits the result into typed settings and functions.
pub(crate) fn load_sources(
    sandbox: &Sandbox,
    sources: &[(&'static str, Source)],
    chunks: &[(String, String)],
) -> Result<(Table, Settings, Vec<String>), RuleError> {
    let lua = &sandbox.lua;
    let lua_err = |e: mlua::Error| RuleError::from_lua(&e, chunks);
    let base = lua.create_table().map_err(lua_err)?;
    for (id, source) in sources {
        sandbox.budget.reset();
        let function = lua
            .load(source.text.as_str())
            .set_name(format!("={id}"))
            .set_mode(ChunkMode::Text)
            .into_function()
            .map_err(lua_err)?;
        // The file gets a copy: merging its answer into `base` afterwards
        // must not change what `base.risk` means inside the file itself.
        let snapshot = copy_table(lua, &base, 0).map_err(lua_err)?;
        let returned: Value = function.call(snapshot).map_err(lua_err)?;
        let Value::Table(table) = returned else {
            return Err(RuleError {
                file: Some(source.name.clone()),
                line: None,
                function: None,
                message: format!(
                    "a rules file must return a table, it returned {}",
                    describe(Some(&returned))
                ),
            });
        };
        merge(&base, &table, true).map_err(lua_err)?;
    }

    let settings_table = lua.create_table().map_err(lua_err)?;
    let mut warnings = Vec::new();
    for pair in base.pairs::<Value, Value>() {
        let (key, value) = pair.map_err(lua_err)?;
        let Value::String(name) = &key else {
            warnings.push("a rules file has a key that is not a name; ignored".to_owned());
            continue;
        };
        let name = name.to_string_lossy();
        let is_function = matches!(value, Value::Function(_));
        if is_function && FUNCTION_KEYS.contains(&name.as_str()) {
            // `auto_merge` is a setting when it is a boolean and a rule when
            // it is a function.
        } else if SETTING_KEYS.contains(&name.as_str()) {
            settings_table.set(name, value).map_err(lua_err)?;
        } else if FUNCTION_KEYS.contains(&name.as_str()) {
            if !matches!(value, Value::Function(_) | Value::Nil) {
                return Err(RuleError::plain(format!(
                    "`{name}` must be a function, it is {}",
                    describe(Some(&value))
                )));
            }
        } else {
            warnings.push(format!(
                "unknown key `{name}` in the rules (a typo?); ignored"
            ));
        }
    }
    let settings: Settings = lua
        .from_value(Value::Table(settings_table))
        .map_err(|e| RuleError::plain(format!("invalid settings: {e}")))?;
    settings
        .validate()
        .map_err(|m| RuleError::plain(format!("invalid settings: {m}")))?;
    Ok((base, settings, warnings))
}

/// A deep copy of a table of settings (functions are shared).
fn copy_table(lua: &Lua, table: &Table, depth: u32) -> mlua::Result<Table> {
    if depth > 16 {
        return Err(mlua::Error::runtime("a rules table is nested too deeply"));
    }
    let copy = lua.create_table()?;
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let value = match value {
            Value::Table(t) => Value::Table(copy_table(lua, &t, depth + 1)?),
            other => other,
        };
        copy.set(key, value)?;
    }
    Ok(copy)
}

/// Merges `over` into `base`: tables that are maps merge key by key, lists
/// and everything else replace; at the top level `editors` merges by name.
fn merge(base: &Table, over: &Table, top: bool) -> mlua::Result<()> {
    for pair in over.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let is_editors =
            top && matches!(&key, Value::String(s) if s.to_string_lossy() == "editors");
        match (base.get::<Value>(key.clone())?, &value) {
            (Value::Table(b), Value::Table(o)) if is_editors => merge_editors(&b, o)?,
            (Value::Table(b), Value::Table(o)) if b.raw_len() == 0 && o.raw_len() == 0 => {
                merge(&b, o, false)?;
            }
            _ => base.set(key, value)?,
        }
    }
    Ok(())
}

fn merge_editors(base: &Table, over: &Table) -> mlua::Result<()> {
    let name_of = |t: &Table| t.get::<Option<String>>("name");
    for editor in over.sequence_values::<Table>() {
        let editor = editor?;
        let name = name_of(&editor)?;
        let mut replaced = false;
        for i in 1..=base.raw_len() {
            let existing: Table = base.get(i)?;
            if name.is_some() && name_of(&existing)? == name {
                base.set(i, editor.clone())?;
                replaced = true;
                break;
            }
        }
        if !replaced {
            base.push(editor)?;
        }
    }
    Ok(())
}

pub(crate) fn change_value(lua: &Lua, change: &Change) -> mlua::Result<Value> {
    lua.to_value(&ChangeView::from(change))
}

/// The arguments of `layers`, `review` and `auto_merge`: the change, the
/// risk level as a word, and the reasons as a list.
pub(crate) fn risk_args(
    lua: &Lua,
    change: &Change,
    risk: &RiskAssessment,
) -> mlua::Result<Vec<Value>> {
    Ok(vec![
        change_value(lua, change)?,
        Value::String(lua.create_string(risk.level.as_str())?),
        lua.to_value(&risk.reasons)?,
    ])
}

pub(crate) fn describe(value: Option<&Value>) -> String {
    value.map_or_else(|| "nothing".to_owned(), |v| v.type_name().to_owned())
}

pub(crate) fn fail<T>(
    value: T,
    name: &str,
    err: &mlua::Error,
    chunks: &[(String, String)],
) -> Evaluated<T> {
    Evaluated {
        value,
        error: Some(RuleError::from_lua(err, chunks).in_function(name)),
    }
}

/// An error about what a function returned, placed at the function's line.
pub(crate) fn located(
    function: &Function,
    name: &str,
    message: String,
    chunks: &[(String, String)],
) -> RuleError {
    let info = function.info();
    let file = info
        .short_src
        .as_deref()
        .and_then(|src| chunks.iter().find(|(id, _)| id == src))
        .map(|(_, file)| file.clone());
    RuleError {
        file,
        line: info.line_defined.and_then(|l| u32::try_from(l).ok()),
        function: Some(name.to_owned()),
        message,
    }
}

fn string_list(value: Option<Value>, what: &str) -> Result<Vec<String>, String> {
    match value {
        None | Some(Value::Nil) => Ok(Vec::new()),
        Some(Value::String(s)) => Ok(vec![s.to_string_lossy()]),
        Some(Value::Table(t)) => t
            .sequence_values::<Value>()
            .map(|v| match v {
                Ok(Value::String(s)) => Ok(s.to_string_lossy()),
                _ => Err(format!("{what} must be a list of strings")),
            })
            .collect(),
        Some(other) => Err(format!(
            "{what} must be a string or a list, got {}",
            other.type_name()
        )),
    }
}

fn level_of(text: Option<String>) -> Result<Risk, String> {
    let text = text.unwrap_or_default();
    Risk::parse(&text)
        .ok_or_else(|| format!("risk must be \"low\", \"medium\" or \"high\", got {text:?}"))
}

pub(crate) fn parse_risk(values: MultiValue) -> Result<RiskAssessment, String> {
    let mut it = values.into_iter();
    match it.next() {
        Some(Value::String(s)) => Ok(RiskAssessment {
            level: level_of(Some(s.to_string_lossy()))?,
            reasons: string_list(it.next(), "the reasons")?,
        }),
        Some(Value::Table(t)) => Ok(RiskAssessment {
            level: level_of(
                t.get::<Option<String>>("level")
                    .map_err(|e| e.to_string())?,
            )?,
            reasons: string_list(
                t.get::<Option<Value>>("reasons")
                    .map_err(|e| e.to_string())?,
                "the reasons",
            )?,
        }),
        other => Err(format!(
            "risk must return \"low\", \"medium\" or \"high\", got {}",
            describe(other.as_ref())
        )),
    }
}

pub(crate) fn parse_layers(values: MultiValue) -> Result<Vec<Layer>, String> {
    let names = string_list(values.into_iter().next(), "layers")?;
    let mut layers = Vec::new();
    for name in names {
        let layer = Layer::parse(&name).ok_or_else(|| {
            format!("unknown layer {name:?} (summary, plan, review, diagram, diff)")
        })?;
        if !layers.contains(&layer) {
            layers.push(layer);
        }
    }
    Ok(layers)
}

pub(crate) fn parse_review(
    values: MultiValue,
    fallback: &ReviewPlan,
) -> Result<ReviewPlan, String> {
    let Some(Value::Table(t)) = values.into_iter().next() else {
        return Err("review must return a table { model = ..., reviewers = ... }".to_owned());
    };
    let model = t
        .get::<Option<String>>("model")
        .map_err(|_| "review.model must be a string".to_owned())?
        .unwrap_or_else(|| fallback.model.clone());
    let reviewers = match t
        .get::<Option<i64>>("reviewers")
        .map_err(|_| "review.reviewers must be a whole number".to_owned())?
    {
        None => fallback.reviewers,
        Some(n) => {
            u8::try_from(n).map_err(|_| "review.reviewers must be between 0 and 255".to_owned())?
        }
    };
    Ok(ReviewPlan { model, reviewers })
}

pub(crate) fn parse_merge(values: MultiValue) -> Result<MergeDecision, String> {
    let mut it = values.into_iter();
    let Some(Value::Boolean(allowed)) = it.next() else {
        return Err("auto_merge must return true or false, and optionally a reason".to_owned());
    };
    let reason = match it.next() {
        Some(Value::String(s)) => s.to_string_lossy(),
        _ if allowed => "allowed by the auto_merge rule".to_owned(),
        _ => "refused by the auto_merge rule".to_owned(),
    };
    Ok(MergeDecision { allowed, reason })
}
