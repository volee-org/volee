//! The Lua rules of volee (v2, step 8).
//!
//! Rust owns the facts and the effects; Lua only decides. A rules file is a
//! Lua chunk that returns a table: readable settings (sensitive paths, size
//! thresholds, review model, editor commands...) and, for special cases,
//! optional functions (`risk`, `layers`, `review`, `auto_merge`, `finished`).
//! Whatever a file leaves out is computed in Rust from its settings, so most
//! people never write Lua.
//!
//! ```
//! use volee_rules::{Change, Risk, Rules, Source};
//!
//! let repo = Source::new(
//!     "rules.lua",
//!     r#"return { sensitive_paths = { "src/billing/**" }, auto_merge = true }"#,
//! );
//! let rules = Rules::from_sources(Some(repo), None);
//! let change = Change::example("src/billing/invoice.rs", 3, 1);
//! assert_eq!(rules.risk(&change).value.level, Risk::High);
//! assert!(rules.errors().is_empty());
//! ```
//!
//! Safety first: a file that fails to load degrades the whole rule set to
//! the safest outcome, and a function that fails at run time makes that one
//! answer the safest one. Every failure is reported with its file and line
//! ([`RuleError`]) and none is silent. See the ADR "Lua config and rules" and
//! `docs/src/reference/rules.md`.

mod data;
mod defaults;
mod editor;
mod engine;
mod error;
mod glob;
mod lua_io;
mod sandbox;
mod settings;
mod types;

pub use data::load_data;
pub use editor::{Editor, EditorError, EditorKind, EditorTarget};
pub use engine::{Evaluated, Evaluation, PERSONAL_RULES_FILE, REPO_RULES_PATH, Rules, Source};
pub use error::RuleError;
pub use glob::glob_match;
pub use sandbox::Limits;
pub use settings::{DurationSetting, LayersSetting, ReviewSetting, Settings, SizeSetting};
pub use types::{
    Change, FileChange, FinishedFacts, Layer, MergeDecision, ReviewPlan, Risk, RiskAssessment,
};
