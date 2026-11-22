//! What the rules answer when a file defines no function: computed in Rust
//! from the settings. The safest outcome is here too.

use crate::glob::glob_match;
use crate::settings::Settings;
use crate::types::{Change, FinishedFacts, Layer, MergeDecision, ReviewPlan, Risk, RiskAssessment};

const MAX_REASONS: usize = 5;

fn matching<'a>(change: &'a Change, globs: &[String]) -> Vec<&'a str> {
    change
        .files
        .iter()
        .map(|f| f.path.as_str())
        .filter(|path| globs.iter().any(|g| glob_match(g, path)))
        .collect()
}

fn listed(prefix: &str, paths: &[&str]) -> String {
    let shown: Vec<&str> = paths.iter().take(3).copied().collect();
    let more = paths.len() - shown.len();
    if more > 0 {
        format!("{prefix}: {} and {more} more", shown.join(", "))
    } else {
        format!("{prefix}: {}", shown.join(", "))
    }
}

/// The default risk: sensitive paths, migrations, dependency files, size and
/// untested code, each from the settings.
pub(crate) fn risk(settings: &Settings, change: &Change) -> RiskAssessment {
    let mut level = Risk::Low;
    let mut reasons: Vec<String> = Vec::new();
    let mut raise = |to: Risk, why: String| {
        level = level.max(to);
        if reasons.len() < MAX_REASONS {
            reasons.push(why);
        }
    };

    let sensitive = matching(change, &settings.sensitive_paths);
    if !sensitive.is_empty() {
        raise(Risk::High, listed("sensitive path", &sensitive));
    }
    let migrations = matching(change, &settings.migration_paths);
    if change.migrations || !migrations.is_empty() {
        raise(Risk::High, listed("database migration", &migrations));
    }
    let dependencies = matching(change, &settings.dependency_paths);
    if !dependencies.is_empty() {
        raise(Risk::Medium, listed("dependency file", &dependencies));
    }

    let size = &settings.size;
    let lines = change.lines_changed();
    let files = u32::try_from(change.files.len()).unwrap_or(u32::MAX);
    if lines >= size.high_lines {
        raise(Risk::High, format!("{lines} lines changed"));
    } else if lines >= size.medium_lines {
        raise(Risk::Medium, format!("{lines} lines changed"));
    }
    if files >= size.high_files {
        raise(Risk::High, format!("{files} files changed"));
    } else if files >= size.medium_files {
        raise(Risk::Medium, format!("{files} files changed"));
    }
    if !change.tests_touched && lines >= size.untested_lines {
        raise(
            Risk::Medium,
            format!("{lines} lines changed and no test touched"),
        );
    }

    if reasons.is_empty() {
        reasons.push("small change, nothing sensitive".to_owned());
    }
    RiskAssessment { level, reasons }
}

/// The default layers for a risk level.
pub(crate) fn layers(settings: &Settings, risk: Risk) -> Vec<Layer> {
    match risk {
        Risk::Low => settings.layers.low.clone(),
        Risk::Medium => settings.layers.medium.clone(),
        Risk::High => settings.layers.high.clone(),
    }
}

/// The default review: the configured model, more reviewers when high risk.
pub(crate) fn review(settings: &Settings, risk: Risk) -> ReviewPlan {
    let reviewers = if risk == Risk::High {
        settings.review.high_reviewers
    } else {
        settings.review.reviewers
    };
    ReviewPlan {
        model: settings.review.model.clone(),
        reviewers,
    }
}

/// The default merge rule: automatic only when the settings allow it and the
/// risk is low.
pub(crate) fn auto_merge(settings: &Settings, risk: Risk) -> MergeDecision {
    if !settings.auto_merge {
        MergeDecision {
            allowed: false,
            reason: "automatic merge is off".into(),
        }
    } else if risk == Risk::Low {
        MergeDecision {
            allowed: true,
            reason: "low risk and automatic merge is on".into(),
        }
    } else {
        MergeDecision {
            allowed: false,
            reason: format!("{} risk needs a person", risk.as_str()),
        }
    }
}

/// The default finished-work rule, the same as `volee_core::finished`: the
/// agent stopped after its last activity, is not busy, and its branch holds
/// unique commits its base does not have.
pub(crate) fn finished(facts: &FinishedFacts) -> bool {
    !facts.busy
        && facts.stopped_last
        && !facts.merged
        && facts.commits > 0
        && facts.unique_commits > 0
}

/// The outcome after a failure: high risk, no automatic merge, every layer,
/// at least two reviewers.
pub(crate) mod safest {
    use super::{Layer, MergeDecision, ReviewPlan, Risk, RiskAssessment, Settings};

    pub(crate) fn risk(why: &str) -> RiskAssessment {
        RiskAssessment {
            level: Risk::High,
            reasons: vec![format!("rules failed, safest outcome: {why}")],
        }
    }

    pub(crate) fn layers() -> Vec<Layer> {
        Layer::ALL.to_vec()
    }

    pub(crate) fn review(settings: &Settings) -> ReviewPlan {
        ReviewPlan {
            model: settings.review.model.clone(),
            reviewers: settings.review.high_reviewers.max(2),
        }
    }

    pub(crate) fn auto_merge() -> MergeDecision {
        MergeDecision {
            allowed: false,
            reason: "rules failed, safest outcome: a person merges".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FileChange;

    fn change(paths: &[(&str, u32, u32)]) -> Change {
        Change {
            files: paths
                .iter()
                .map(|(p, a, r)| FileChange {
                    path: (*p).into(),
                    added: *a,
                    removed: *r,
                })
                .collect(),
            tests_touched: true,
            ..Change::default()
        }
    }

    #[test]
    fn a_small_clean_change_is_low() {
        let r = risk(&Settings::default(), &change(&[("src/a.rs", 10, 2)]));
        assert_eq!(r.level, Risk::Low);
        assert_eq!(r.reasons, ["small change, nothing sensitive"]);
    }

    #[test]
    fn migrations_and_sensitive_paths_are_high() {
        let s = Settings {
            sensitive_paths: vec!["src/auth/**".into()],
            ..Settings::default()
        };
        assert_eq!(
            risk(&s, &change(&[("src/auth/x.rs", 1, 0)])).level,
            Risk::High
        );
        assert_eq!(
            risk(&s, &change(&[("db/migrations/1.sql", 1, 0)])).level,
            Risk::High
        );
        let mut flagged = change(&[("src/a.rs", 1, 0)]);
        flagged.migrations = true;
        assert_eq!(risk(&s, &flagged).level, Risk::High);
    }

    #[test]
    fn dependencies_size_and_missing_tests_raise_to_medium_or_high() {
        let s = Settings::default();
        assert_eq!(
            risk(&s, &change(&[("Cargo.lock", 3, 3)])).level,
            Risk::Medium
        );
        assert_eq!(risk(&s, &change(&[("a.rs", 150, 60)])).level, Risk::Medium);
        assert_eq!(risk(&s, &change(&[("a.rs", 700, 200)])).level, Risk::High);
        let mut untested = change(&[("a.rs", 60, 0)]);
        untested.tests_touched = false;
        assert_eq!(risk(&s, &untested).level, Risk::Medium);
    }

    #[test]
    fn auto_merge_needs_the_setting_and_low_risk() {
        let mut s = Settings::default();
        assert!(!auto_merge(&s, Risk::Low).allowed);
        s.auto_merge = true;
        assert!(auto_merge(&s, Risk::Low).allowed);
        assert!(!auto_merge(&s, Risk::Medium).allowed);
    }

    #[test]
    fn safest_has_two_reviewers_even_when_settings_say_fewer() {
        let mut s = Settings::default();
        s.review.high_reviewers = 0;
        assert_eq!(safest::review(&s).reviewers, 2);
    }
}
