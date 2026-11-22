//! A Lua file that returns plain data: volee's configuration file.
//!
//! It runs in the same sandbox as the rules (same libraries, same limits),
//! and its table is read into a Rust type. Functions in it are an error:
//! the configuration is data, the rules are the place for decisions.

use mlua::chunk::ChunkMode;
use mlua::{LuaSerdeExt, Value};
use serde::de::DeserializeOwned;

use crate::engine::Source;
use crate::error::RuleError;
use crate::sandbox::{Limits, Sandbox};

const DATA_CHUNK: &str = "volee-data";

/// Runs `source`, which must return a table, and reads that table into `T`.
/// Errors carry the file and line.
pub fn load_data<T: DeserializeOwned>(source: &Source, limits: Limits) -> Result<T, RuleError> {
    let chunks = [(DATA_CHUNK.to_owned(), source.name.clone())];
    let lua_err = |e: mlua::Error| RuleError::from_lua(&e, &chunks);
    let sandbox = Sandbox::new(limits)
        .map_err(|e| RuleError::plain(format!("cannot start the Lua sandbox: {e}")))?;
    let lua = &sandbox.lua;
    sandbox.budget.reset();
    let function = lua
        .load(source.text.as_str())
        .set_name(format!("={DATA_CHUNK}"))
        .set_mode(ChunkMode::Text)
        .into_function()
        .map_err(lua_err)?;
    let returned: Value = function.call(()).map_err(lua_err)?;
    let Value::Table(table) = returned else {
        return Err(RuleError {
            file: Some(source.name.clone()),
            line: None,
            function: None,
            message: "the file must return a table".to_owned(),
        });
    };
    lua.from_value(Value::Table(table)).map_err(|e| RuleError {
        file: Some(source.name.clone()),
        line: None,
        function: None,
        message: first_line(&e),
    })
}

/// The first line of an error: mlua adds a traceback to the ones it wraps.
fn first_line(error: &mlua::Error) -> String {
    let text = error.to_string();
    text.lines()
        .next()
        .unwrap_or_default()
        .trim_start_matches("runtime error: ")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Default, Deserialize, PartialEq)]
    #[serde(default, deny_unknown_fields)]
    struct Sample {
        port: u16,
        names: Vec<String>,
        nested: std::collections::BTreeMap<String, u8>,
    }

    fn load(text: &str) -> Result<Sample, RuleError> {
        load_data(&Source::new("config.lua", text), Limits::default())
    }

    #[test]
    fn a_table_is_read_into_a_type_and_empty_tables_are_empty_lists_or_maps() {
        let sample = load("return { port = 4000, names = {}, nested = {} }").unwrap();
        assert_eq!(sample.port, 4000);
        assert!(sample.names.is_empty() && sample.nested.is_empty());
        let sample = load("return { names = { 'a', 'b' }, nested = { x = 1 } }").unwrap();
        assert_eq!(sample.names, ["a", "b"]);
        assert_eq!(sample.nested["x"], 1);
    }

    #[test]
    fn lua_can_compute_values() {
        let sample = load("local base = 4000\nreturn { port = base + 1 }").unwrap();
        assert_eq!(sample.port, 4001);
    }

    #[test]
    fn mistakes_say_what_and_where() {
        let error = load("local t = {}\nreturn t.x.y").unwrap_err();
        assert_eq!(error.file.as_deref(), Some("config.lua"), "{error}");
        assert_eq!(error.line, Some(2), "{error}");
        let error = load("return { prot = 1 }").unwrap_err();
        assert!(error.message.contains("prot"), "{error}");
        let error = load("return { port = 70000 }").unwrap_err();
        assert_eq!(error.file.as_deref(), Some("config.lua"));
        assert!(load("return 3").unwrap_err().message.contains("table"));
    }

    #[test]
    fn functions_and_the_machine_are_out_of_reach() {
        assert!(load("return { port = function() end }").is_err());
        assert!(load("return { port = io.open('x') }").is_err());
        assert!(load("while true do end").is_err());
    }
}
