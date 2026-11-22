//! The risk of a finished change, as the card shows it (v2, step 8): what the
//! rules computed for the branch tip, what the person set by hand (kept per
//! tip, shown as overridden), the layers that follow from the level, and
//! whether the change may merge by itself.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use serde::Serialize;
use serde_json::json;
use volee_rules::{Layer, Risk, RiskAssessment, RuleError};
use volee_store::Store;

use crate::review::{Target, change_of};
use crate::rules::RulesService;

/// The automatic merge decision, in words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct MergeView {
    pub(crate) allowed: bool,
    pub(crate) reason: String,
}

/// What a card says about the risk of its change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct RiskView {
    /// The branch tip this is about.
    pub(crate) tip: String,
    /// The level in force: the person's, else the rules'.
    pub(crate) level: Risk,
    /// Why: the rules' reasons, or that the person chose it.
    pub(crate) reasons: Vec<String>,
    /// What the rules computed.
    pub(crate) computed: Risk,
    /// The person set `level` by hand for this tip.
    pub(crate) overridden: bool,
    /// The layers the screen opens with, in reading order.
    pub(crate) layers: Vec<Layer>,
    /// Whether volee may merge by itself, and why not.
    pub(crate) auto_merge: MergeView,
    /// What the repository's rules asked for and yours refused.
    pub(crate) notes: Vec<String>,
    /// Rules that failed: the answer is then the safest one.
    pub(crate) errors: Vec<String>,
}

/// The risk of branches, with the person's overrides.
pub(crate) struct Assessments {
    rules: Arc<RulesService>,
    store: Mutex<Store>,
}

impl std::fmt::Debug for Assessments {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Assessments").finish_non_exhaustive()
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

fn describe(error: &RuleError) -> String {
    error.to_string()
}

impl Assessments {
    pub(crate) fn open(rules: Arc<RulesService>, database: &Path) -> Result<Self, String> {
        let store = Store::open(database).map_err(|error| error.to_string())?;
        Ok(Self {
            rules,
            store: Mutex::new(store),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The risk of `target`'s branch at its current tip. Blocking (git, the
    /// rules); cached per tip while the rules files stay the same.
    pub(crate) fn assess(&self, target: &Target) -> Option<RiskView> {
        let tip = target.tip()?;
        let change = change_of(target, &tip);
        let report = self.rules.evaluate(&target.repo_root, &tip, &change);
        let evaluation = &report.evaluation;
        let repo = target.repo_key();
        let chosen = self
            .lock()
            .risk_override(&repo, &tip)
            .map_err(|error| tracing::warn!(%error, "cannot read the risk the person set"))
            .ok()
            .flatten()
            .and_then(|word| match word.as_str() {
                "low" => Some(Risk::Low),
                "medium" => Some(Risk::Medium),
                "high" => Some(Risk::High),
                _ => None,
            });
        let mut view = RiskView {
            tip,
            level: evaluation.risk.level,
            reasons: evaluation.risk.reasons.clone(),
            computed: evaluation.risk.level,
            overridden: false,
            layers: evaluation.layers.clone(),
            auto_merge: MergeView {
                allowed: evaluation.merge.allowed,
                reason: evaluation.merge.reason.clone(),
            },
            notes: evaluation.notes.clone(),
            errors: evaluation.errors.iter().map(describe).collect(),
        };
        if let Some(level) = chosen.filter(|level| *level != evaluation.risk.level) {
            let by_hand = RiskAssessment {
                level,
                reasons: vec![format!(
                    "set by you (the rules said {})",
                    evaluation.risk.level.as_str()
                )],
            };
            view.layers = self.rules.layers(&target.repo_root, &change, &by_hand);
            view.level = level;
            view.reasons = by_hand.reasons;
            view.overridden = true;
            // A person who lowered the risk did not ask for an automatic
            // merge: only the rules' own decision at the rules' own level
            // stands, and never at high.
            if level == Risk::High {
                view.auto_merge = MergeView {
                    allowed: false,
                    reason: "high risk is never merged automatically".to_owned(),
                };
            }
        }
        Some(view)
    }

    /// Sets (`Some`) or takes back (`None`) the person's risk for the
    /// branch's current tip. `None` when the branch has no tip.
    pub(crate) fn set_override(&self, target: &Target, level: Option<Risk>) -> Option<RiskView> {
        let tip = target.tip()?;
        let stored = level.map(Risk::as_str);
        if let Err(error) = self
            .lock()
            .set_risk_override(&target.repo_key(), &tip, stored, now())
        {
            tracing::warn!(%error, "cannot keep the risk the person set");
        }
        self.rules.notify();
        self.assess(target)
    }

    /// The rules service these assessments use.
    pub(crate) fn rules(&self) -> &Arc<RulesService> {
        &self.rules
    }
}

/// A rule failure as the interface shows it.
pub(crate) fn error_json(error: &RuleError) -> serde_json::Value {
    json!({
        "file": error.file,
        "line": error.line,
        "function": error.function,
        "message": error.message,
        "text": error.to_string(),
    })
}
