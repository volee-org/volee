//! The Lua state: what a rules file may use, and how long and how much.
//!
//! Lua 5.4, vendored. Only `table`, `string`, `utf8`, `math` and a three
//! function `os` are loaded. There is no `io`, `package`, `require`, `debug`,
//! `coroutine`, `load`, `dofile`, `loadfile`, `string.dump` or
//! `collectgarbage`. Every call runs under an instruction count, a wall-clock
//! budget and a memory ceiling; the hook that enforces them also survives
//! `pcall` (see [`Budget`]).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use mlua::{
    Function, HookTriggers, Lua, LuaOptions, LuaSerdeExt, MultiValue, StdLib, Table, Value, VmState,
};

use crate::defaults;
use crate::glob::glob_match;
use crate::settings::Settings;
use crate::types::{Change, Risk};

/// How much a rule call may use. These are not settings: a rules file cannot
/// raise its own limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Lua VM instructions per call (the file's top level counts as a call).
    pub instructions: u64,
    /// Wall-clock time per call.
    pub time: Duration,
    /// Memory the rules may allocate on top of what the sandbox itself uses.
    pub memory_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            instructions: 10_000_000,
            time: Duration::from_millis(500),
            memory_bytes: 16 * 1024 * 1024,
        }
    }
}

const HOOK_EVERY: u32 = 1000;

/// The running budget of one call. Once exceeded it stays exceeded until the
/// next call, and the replacement `pcall` refuses to swallow the error, so a
/// script cannot loop forever by catching it.
#[derive(Debug)]
pub(crate) struct Budget {
    limits: Limits,
    started: Mutex<Instant>,
    instructions: AtomicU64,
    exceeded: AtomicBool,
}

impl Budget {
    fn new(limits: Limits) -> Self {
        Self {
            limits,
            started: Mutex::new(Instant::now()),
            instructions: AtomicU64::new(0),
            exceeded: AtomicBool::new(false),
        }
    }

    pub(crate) fn reset(&self) {
        *self.started.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now();
        self.instructions.store(0, Ordering::Relaxed);
        self.exceeded.store(false, Ordering::Relaxed);
    }

    fn is_exceeded(&self) -> bool {
        self.exceeded.load(Ordering::Relaxed)
    }

    fn tick(&self) -> Option<&'static str> {
        let used = self
            .instructions
            .fetch_add(u64::from(HOOK_EVERY), Ordering::Relaxed);
        let elapsed = self
            .started
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .elapsed();
        let why = if used >= self.limits.instructions {
            Some("the rule ran too many instructions (infinite loop?)")
        } else if elapsed > self.limits.time {
            Some("the rule took too long")
        } else {
            None
        };
        if why.is_some() {
            self.exceeded.store(true, Ordering::Relaxed);
        }
        why
    }
}

pub(crate) type SharedSettings = Arc<Mutex<Arc<Settings>>>;

/// A sandboxed Lua state with the `volee` helper table.
pub(crate) struct Sandbox {
    pub(crate) lua: Lua,
    pub(crate) budget: Arc<Budget>,
    pub(crate) settings: SharedSettings,
}

impl std::fmt::Debug for Sandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sandbox").finish_non_exhaustive()
    }
}

impl Sandbox {
    pub(crate) fn new(limits: Limits) -> mlua::Result<Self> {
        let lua = Lua::new_with(
            StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::MATH | StdLib::OS,
            LuaOptions::default(),
        )?;
        let budget = Arc::new(Budget::new(limits));
        let settings: SharedSettings = Arc::new(Mutex::new(Arc::new(Settings::default())));

        strip_globals(&lua, &budget)?;
        install_volee(&lua, &settings)?;

        let hook_budget = Arc::clone(&budget);
        lua.set_hook(
            HookTriggers::new().every_nth_instruction(HOOK_EVERY),
            move |_, debug| match hook_budget.tick() {
                Some(why) => {
                    // Say where, in the form Lua gives its own errors.
                    let source = debug.source();
                    let chunk = source.short_src.as_deref().unwrap_or("?");
                    let line = debug.current_line().unwrap_or(0);
                    Err(mlua::Error::runtime(format!("{chunk}:{line}: {why}")))
                }
                None => Ok(VmState::Continue),
            },
        )?;
        lua.set_memory_limit(lua.used_memory() + limits.memory_bytes)?;
        Ok(Self {
            lua,
            budget,
            settings,
        })
    }

    pub(crate) fn publish_settings(&self, settings: Settings) {
        *self.settings.lock().unwrap_or_else(PoisonError::into_inner) = Arc::new(settings);
    }
}

/// Removes what a rules file must not reach and replaces `os`, `pcall`, `print`.
fn strip_globals(lua: &Lua, budget: &Arc<Budget>) -> mlua::Result<()> {
    let globals = lua.globals();
    for name in [
        "dofile",
        "loadfile",
        "load",
        "loadstring",
        "require",
        "collectgarbage",
        "xpcall",
        "package",
        "io",
        "debug",
        "coroutine",
    ] {
        globals.set(name, Value::Nil)?;
    }
    let string: Table = globals.get("string")?;
    string.set("dump", Value::Nil)?;

    // os: time, clock and date only.
    let os: Table = globals.get("os")?;
    let safe_os = lua.create_table()?;
    for name in ["time", "clock", "date"] {
        safe_os.set(name, os.get::<Value>(name)?)?;
    }
    globals.set("os", safe_os)?;

    // print does nothing: rules decide, they do not talk to the terminal.
    globals.set("print", lua.create_function(|_, _: MultiValue| Ok(()))?)?;

    // pcall that never swallows an exceeded budget.
    let budget = Arc::clone(budget);
    let pcall = lua.create_function(move |lua, (f, args): (Function, MultiValue)| match f
        .call::<MultiValue>(args)
    {
        Ok(mut values) => {
            values.push_front(Value::Boolean(true));
            Ok(values)
        }
        Err(err) if budget.is_exceeded() => Err(err),
        Err(err) => {
            let message = match &err {
                mlua::Error::RuntimeError(text) => text.clone(),
                mlua::Error::CallbackError { cause, .. } => cause.to_string(),
                other => other.to_string(),
            };
            Ok(MultiValue::from_iter([
                Value::Boolean(false),
                Value::String(lua.create_string(message)?),
            ]))
        }
    })?;
    globals.set("pcall", pcall)?;
    Ok(())
}

/// The `volee` table: `api`, `glob`, `touches`, `settings`, `default`.
fn install_volee(lua: &Lua, shared: &SharedSettings) -> mlua::Result<()> {
    let volee = lua.create_table()?;
    volee.set("api", 1)?;

    volee.set(
        "glob",
        lua.create_function(|_, (pattern, path): (String, String)| {
            Ok(glob_match(&pattern, &path))
        })?,
    )?;

    // volee.touches(change, "glob" | {"glob", ...}) -> first matching path or nil
    volee.set(
        "touches",
        lua.create_function(|lua, (change, globs): (Value, Value)| {
            let change: Change = lua.from_value(change)?;
            let globs = string_list(globs)?;
            Ok(change
                .files
                .iter()
                .find(|f| globs.iter().any(|g| glob_match(g, &f.path)))
                .map(|f| f.path.clone()))
        })?,
    )?;

    let settings_cell = Arc::clone(shared);
    volee.set(
        "settings",
        lua.create_function(move |lua, ()| {
            let settings = current(&settings_cell);
            lua.to_value(&*settings)
        })?,
    )?;

    // volee.default.<rule>: what Rust computes from the settings, so a rule
    // can start from it and adjust.
    let default = lua.create_table()?;
    let cell = Arc::clone(shared);
    default.set(
        "risk",
        lua.create_function(move |lua, change: Value| {
            let change: Change = lua.from_value(change)?;
            let r = defaults::risk(&current(&cell), &change);
            Ok((r.level.as_str().to_owned(), r.reasons))
        })?,
    )?;
    let cell = Arc::clone(shared);
    default.set(
        "layers",
        lua.create_function(move |_, risk: String| {
            let risk = parse_risk(&risk)?;
            Ok(defaults::layers(&current(&cell), risk)
                .into_iter()
                .map(|l| l.as_str().to_owned())
                .collect::<Vec<_>>())
        })?,
    )?;
    let cell = Arc::clone(shared);
    default.set(
        "review",
        lua.create_function(move |lua, risk: String| {
            let risk = parse_risk(&risk)?;
            lua.to_value(&defaults::review(&current(&cell), risk))
        })?,
    )?;
    let cell = Arc::clone(shared);
    default.set(
        "auto_merge",
        lua.create_function(move |_, risk: String| {
            let risk = parse_risk(&risk)?;
            let d = defaults::auto_merge(&current(&cell), risk);
            Ok((d.allowed, d.reason))
        })?,
    )?;
    volee.set("default", default)?;

    lua.globals().set("volee", volee)?;
    Ok(())
}

fn current(cell: &SharedSettings) -> Arc<Settings> {
    Arc::clone(&cell.lock().unwrap_or_else(PoisonError::into_inner))
}

fn parse_risk(text: &str) -> mlua::Result<Risk> {
    Risk::parse(text).ok_or_else(|| {
        mlua::Error::runtime(format!(
            "risk must be \"low\", \"medium\" or \"high\", got {text:?}"
        ))
    })
}

fn string_list(value: Value) -> mlua::Result<Vec<String>> {
    match value {
        Value::String(s) => Ok(vec![s.to_str()?.to_owned()]),
        Value::Table(t) => t.sequence_values::<String>().collect(),
        other => Err(mlua::Error::runtime(format!(
            "expected a glob or a list of globs, got {}",
            other.type_name()
        ))),
    }
}
