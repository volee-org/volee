//! Loading the rules files and asking them questions.

use std::path::Path;
use std::sync::Arc;

use mlua::{Lua, LuaSerdeExt, MultiValue, Table, Value};

use crate::defaults::{self, safest};
use crate::editor::Editor;
use crate::error::RuleError;
use crate::lua_io::{
    change_value, describe, fail, load_sources, located, parse_layers, parse_merge, parse_review,
    parse_risk, read_source, risk_args,
};
use crate::sandbox::{Limits, Sandbox};
use crate::settings::Settings;
use crate::types::{Change, FinishedFacts, Layer, MergeDecision, ReviewPlan, Risk, RiskAssessment};

/// Where a repository keeps its rules, relative to its root.
pub const REPO_RULES_PATH: &str = ".volee/rules.lua";
/// The name of the personal rules file inside volee's config directory.
pub const PERSONAL_RULES_FILE: &str = "rules.lua";

const REPO_CHUNK: &str = "volee-repo";
const PERSONAL_CHUNK: &str = "volee-personal";

/// A rules file's text and the name shown in errors (normally its path).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// The name shown in errors.
    pub name: String,
    /// The Lua text.
    pub text: String,
}

impl Source {
    /// A source from a name and its text.
    pub fn new(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            text: text.into(),
        }
    }
}

/// An answer, and the error that made it a fallback if there was one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evaluated<T> {
    /// The answer: the rule's, the default's, or the safest outcome.
    pub value: T,
    /// Set when the answer is the safest outcome because something failed.
    pub error: Option<RuleError>,
}

/// All the answers about one change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evaluation {
    /// The risk level and its reasons.
    pub risk: RiskAssessment,
    /// The card layers to show.
    pub layers: Vec<Layer>,
    /// The pre-review to run.
    pub review: ReviewPlan,
    /// Whether to merge without asking.
    pub merge: MergeDecision,
    /// Every failure met on the way (each answer it touched is the safest).
    pub errors: Vec<RuleError>,
}

/// The loaded rules: a repository file and a personal file merged, in one
/// sandboxed Lua state.
///
/// **Merge order**: built-in defaults, then the repository file, then the
/// personal file. Scalars and functions of a later file replace the earlier
/// ones; tables (`size`, `review`, `layers`) merge key by key; lists replace
/// the earlier list, except `editors`, which merges by `name`. The personal
/// file receives the table merged so far as its first argument (`local base
/// = ...`) to extend lists or wrap functions.
#[derive(Debug)]
pub struct Rules {
    sandbox: Option<Sandbox>,
    config: Option<Table>,
    settings: Arc<Settings>,
    load_errors: Vec<RuleError>,
    warnings: Vec<String>,
    chunks: Vec<(String, String)>,
}

impl Rules {
    /// Reads `<repo_root>/.volee/rules.lua` and `<config_dir>/rules.lua`.
    /// A missing file is not an error; an unreadable one is.
    pub fn load(repo_root: Option<&Path>, config_dir: Option<&Path>) -> Self {
        Self::load_with(repo_root, config_dir, Limits::default())
    }

    /// [`Rules::load`] with explicit limits.
    pub fn load_with(repo_root: Option<&Path>, config_dir: Option<&Path>, limits: Limits) -> Self {
        let mut errors = Vec::new();
        let mut read = |path: Option<std::path::PathBuf>| match path.map(|p| read_source(&p)) {
            Some(Ok(source)) => source,
            Some(Err(e)) => {
                errors.push(e);
                None
            }
            None => None,
        };
        let repo = read(repo_root.map(|r| r.join(REPO_RULES_PATH)));
        let personal = read(config_dir.map(|d| d.join(PERSONAL_RULES_FILE)));
        if errors.is_empty() {
            Self::from_sources_with(repo, personal, limits)
        } else {
            Self::degraded(None, errors)
        }
    }

    /// Rules from texts already in memory.
    pub fn from_sources(repo: Option<Source>, personal: Option<Source>) -> Self {
        Self::from_sources_with(repo, personal, Limits::default())
    }

    /// [`Rules::from_sources`] with explicit limits.
    pub fn from_sources_with(
        repo: Option<Source>,
        personal: Option<Source>,
        limits: Limits,
    ) -> Self {
        let sandbox = match Sandbox::new(limits) {
            Ok(sandbox) => sandbox,
            Err(e) => {
                let error = RuleError::plain(format!("cannot start the Lua sandbox: {e}"));
                return Self::degraded(None, vec![error]);
            }
        };
        let mut sources = Vec::new();
        if let Some(s) = repo {
            sources.push((REPO_CHUNK, s));
        }
        if let Some(s) = personal {
            sources.push((PERSONAL_CHUNK, s));
        }
        let chunks: Vec<(String, String)> = sources
            .iter()
            .map(|(id, s)| ((*id).to_owned(), s.name.clone()))
            .collect();
        match load_sources(&sandbox, &sources, &chunks) {
            Ok((config, settings, warnings)) => {
                sandbox.publish_settings(settings.clone());
                Self {
                    sandbox: Some(sandbox),
                    config: Some(config),
                    settings: Arc::new(settings),
                    load_errors: Vec::new(),
                    warnings,
                    chunks,
                }
            }
            Err(error) => Self::degraded(Some(sandbox), vec![error]),
        }
    }

    fn degraded(sandbox: Option<Sandbox>, errors: Vec<RuleError>) -> Self {
        Self {
            sandbox,
            config: None,
            settings: Arc::new(Settings::default()),
            load_errors: errors,
            warnings: Vec::new(),
            chunks: Vec::new(),
        }
    }

    /// Errors met while loading. When there are any, every answer is the
    /// safest outcome and the settings are the built-in defaults.
    pub fn errors(&self) -> &[RuleError] {
        &self.load_errors
    }

    /// Things that are not errors but probably typos (an unknown key).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Whether loading failed and the rules answer the safest outcome.
    pub fn is_degraded(&self) -> bool {
        !self.load_errors.is_empty()
    }

    /// The merged settings (the defaults when loading failed).
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The configured editors.
    pub fn editors(&self) -> &[Editor] {
        &self.settings.editors
    }

    /// The risk of a change. The `risk(change)` function when defined, else
    /// computed from the settings.
    pub fn risk(&self, change: &Change) -> Evaluated<RiskAssessment> {
        self.run(
            "risk",
            |lua| Ok(vec![change_value(lua, change)?]),
            parse_risk,
            || defaults::risk(&self.settings, change),
            || safest::risk("the risk rule failed"),
        )
    }

    /// The card layers for a change at a risk level.
    pub fn layers(&self, change: &Change, risk: &RiskAssessment) -> Evaluated<Vec<Layer>> {
        self.run(
            "layers",
            |lua| risk_args(lua, change, risk),
            parse_layers,
            || defaults::layers(&self.settings, risk.level),
            safest::layers,
        )
    }

    /// The pre-review for a change at a risk level.
    pub fn review(&self, change: &Change, risk: &RiskAssessment) -> Evaluated<ReviewPlan> {
        let fallback = defaults::review(&self.settings, risk.level);
        self.run(
            "review",
            |lua| risk_args(lua, change, risk),
            |values| parse_review(values, &fallback),
            || defaults::review(&self.settings, risk.level),
            || safest::review(&self.settings),
        )
    }

    /// Whether the change may merge without asking. A high risk change is
    /// never merged automatically, whatever a function says.
    pub fn auto_merge(&self, change: &Change, risk: &RiskAssessment) -> Evaluated<MergeDecision> {
        let mut out = self.run(
            "auto_merge",
            |lua| risk_args(lua, change, risk),
            parse_merge,
            || defaults::auto_merge(&self.settings, risk.level),
            safest::auto_merge,
        );
        if out.value.allowed && risk.level == Risk::High {
            out.value = MergeDecision {
                allowed: false,
                reason: "high risk is never merged automatically".into(),
            };
        }
        out
    }

    /// Whether an agent has finished work for a person to look at. When the
    /// `finished(facts)` function fails, the built-in rule answers (a stray
    /// card costs less than a missed one).
    pub fn finished(&self, facts: &FinishedFacts) -> Evaluated<bool> {
        let mut out = self.run(
            "finished",
            |lua| Ok(vec![lua.to_value(facts)?]),
            |values| match values.into_iter().next() {
                Some(Value::Boolean(b)) => Ok(b),
                other => Err(format!(
                    "finished must return true or false, got {}",
                    describe(other.as_ref())
                )),
            },
            || defaults::finished(facts),
            || defaults::finished(facts),
        );
        if self.is_degraded() {
            out.value = defaults::finished(facts);
        }
        out
    }

    /// Risk, layers, review and merge for one change.
    pub fn evaluate(&self, change: &Change) -> Evaluation {
        let risk = self.risk(change);
        let layers = self.layers(change, &risk.value);
        let review = self.review(change, &risk.value);
        let merge = self.auto_merge(change, &risk.value);
        let errors = if self.is_degraded() {
            self.load_errors.clone()
        } else {
            [risk.error, layers.error, review.error, merge.error]
                .into_iter()
                .flatten()
                .collect()
        };
        Evaluation {
            risk: risk.value,
            layers: layers.value,
            review: review.value,
            merge: merge.value,
            errors,
        }
    }

    fn run<T>(
        &self,
        name: &'static str,
        args: impl FnOnce(&Lua) -> mlua::Result<Vec<Value>>,
        parse: impl FnOnce(MultiValue) -> Result<T, String>,
        default: impl FnOnce() -> T,
        safest: impl FnOnce() -> T,
    ) -> Evaluated<T> {
        let (Some(sandbox), Some(config)) = (&self.sandbox, &self.config) else {
            return Evaluated {
                value: safest(),
                error: self.load_errors.first().cloned(),
            };
        };
        let function = match config.get::<Value>(name) {
            Ok(Value::Function(f)) => f,
            Ok(_) => {
                return Evaluated {
                    value: default(),
                    error: None,
                };
            }
            Err(e) => return fail(safest(), name, &e, &self.chunks),
        };
        sandbox.budget.reset();
        let outcome = args(&sandbox.lua)
            .and_then(|values| function.call::<MultiValue>(MultiValue::from_iter(values)));
        match outcome {
            Ok(values) => match parse(values) {
                Ok(value) => Evaluated { value, error: None },
                Err(message) => Evaluated {
                    value: safest(),
                    error: Some(located(&function, name, message, &self.chunks)),
                },
            },
            Err(e) => fail(safest(), name, &e, &self.chunks),
        }
    }
}
