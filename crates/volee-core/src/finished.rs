//! What counts as finished work (v2, step 2).
//!
//! One question, answered in one place: has this agent stopped and left
//! something for a person to look at? [`is_finished_work`] is that rule. It
//! is deliberately small and takes only facts, so that the rules of step 8
//! (Lua) can replace it without touching what gathers the facts.
//!
//! The facts come from two sources that disagree in speed: Claude Code's
//! hooks say when the agent last did something and when it last stopped; git
//! says what the branch holds that its base does not.

use serde::{Deserialize, Serialize};

use crate::state::AgentState;

/// What a hook event says about the agent's activity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookKind {
    /// The agent did, or was asked to do, something: a prompt, a tool call, a
    /// permission request, a subagent.
    Activity,
    /// The agent stopped: its turn ended, or Claude Code said the agent
    /// completed or sits idle.
    Completion,
    /// Neither: a session starting or ending, a notification of another
    /// kind.
    Other,
}

/// Reads a hook event as activity or completion.
///
/// Subagent and task events are neither: Claude Code runs a subagent after a
/// turn ends (measured on 2.1.330: a `SubagentStop` four seconds after the
/// `Stop`, from the one that writes the suggested next prompt), and a
/// background task can outlast the turn that started it. Counting them as
/// activity would make a finished agent look busy.
///
/// `event` is the hook event name (`Stop`, `PreToolUse`...) and
/// `notification_type` the payload's `notification_type` for a
/// `Notification`, when it has one.
pub fn classify_hook(event: &str, notification_type: Option<&str>) -> HookKind {
    match event {
        "UserPromptSubmit" | "PreToolUse" | "PostToolUse" | "PostToolUseFailure"
        | "PermissionRequest" | "PermissionDenied" => HookKind::Activity,
        "Stop" | "StopFailure" => HookKind::Completion,
        "Notification" => match notification_type {
            Some("agent_completed" | "idle_prompt") => HookKind::Completion,
            Some("permission_prompt" | "agent_needs_input") => HookKind::Activity,
            _ => HookKind::Other,
        },
        _ => HookKind::Other,
    }
}

/// When the agent last did something and when it last stopped, from its hook
/// events, in milliseconds since the Unix epoch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    /// The newest [`HookKind::Activity`] event.
    pub last_activity_at: Option<i64>,
    /// The newest [`HookKind::Completion`] event.
    pub last_stop_at: Option<i64>,
}

impl Activity {
    /// Whether the agent's newest sign of life is a stop: it stopped after
    /// the last thing it did.
    pub fn stopped_last(&self) -> bool {
        match (self.last_stop_at, self.last_activity_at) {
            (Some(stop), Some(activity)) => stop >= activity,
            (Some(_), None) => true,
            (None, _) => false,
        }
    }
}

/// What the agent's branch holds, from git.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchWork {
    /// Commits on the branch that the base branch does not have.
    pub commits: u32,
    /// Of those, the commits whose change is not already in the base branch
    /// as a patch.
    pub unique_commits: u32,
    /// The branch is merged into its base, as far as git can tell.
    pub merged: bool,
}

impl BranchWork {
    /// Whether the branch holds work the base branch does not have.
    pub fn has_unmerged_work(&self) -> bool {
        !self.merged && self.commits > 0 && self.unique_commits > 0
    }
}

/// Finished work: an agent that stopped after its last activity, is not
/// working or waiting for an answer now, and whose branch holds commits its
/// base branch does not have.
pub fn is_finished_work(activity: &Activity, state: &AgentState, work: &BranchWork) -> bool {
    let at_rest = !matches!(
        state,
        AgentState::Working | AgentState::NeedsInput { .. } | AgentState::Launching
    );
    at_rest && activity.stopped_last() && work.has_unmerged_work()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::InputRequest;

    const WORK: BranchWork = BranchWork {
        commits: 2,
        unique_commits: 2,
        merged: false,
    };

    fn activity(last_activity_at: Option<i64>, last_stop_at: Option<i64>) -> Activity {
        Activity {
            last_activity_at,
            last_stop_at,
        }
    }

    #[test]
    fn events_are_read_as_activity_or_completion() {
        for event in [
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "PermissionRequest",
        ] {
            assert_eq!(classify_hook(event, None), HookKind::Activity, "{event}");
        }
        for event in [
            "SubagentStart",
            "SubagentStop",
            "TaskCreated",
            "TaskCompleted",
        ] {
            assert_eq!(classify_hook(event, None), HookKind::Other, "{event}");
        }
        assert_eq!(classify_hook("Stop", None), HookKind::Completion);
        assert_eq!(
            classify_hook("Notification", Some("agent_completed")),
            HookKind::Completion
        );
        assert_eq!(
            classify_hook("Notification", Some("idle_prompt")),
            HookKind::Completion
        );
        assert_eq!(
            classify_hook("Notification", Some("permission_prompt")),
            HookKind::Activity
        );
        assert_eq!(classify_hook("Notification", None), HookKind::Other);
        assert_eq!(classify_hook("SessionStart", None), HookKind::Other);
        assert_eq!(classify_hook("SessionEnd", None), HookKind::Other);
        assert_eq!(classify_hook("SomethingNew", None), HookKind::Other);
    }

    #[test]
    fn work_is_finished_when_the_agent_stopped_after_its_last_activity_with_commits() {
        assert!(is_finished_work(
            &activity(Some(100), Some(200)),
            &AgentState::Idle,
            &WORK
        ));
        assert!(is_finished_work(
            &activity(None, Some(200)),
            &AgentState::Done,
            &WORK
        ));
    }

    #[test]
    fn an_agent_that_acted_after_its_last_stop_is_not_finished() {
        assert!(!is_finished_work(
            &activity(Some(300), Some(200)),
            &AgentState::Idle,
            &WORK
        ));
        assert!(!is_finished_work(
            &activity(Some(300), None),
            &AgentState::Idle,
            &WORK
        ));
    }

    #[test]
    fn an_agent_that_works_or_waits_now_is_not_finished_whatever_its_hooks_say() {
        let stopped = activity(Some(100), Some(200));
        for state in [
            AgentState::Working,
            AgentState::Launching,
            AgentState::NeedsInput {
                request: InputRequest::Question,
            },
        ] {
            assert!(!is_finished_work(&stopped, &state, &WORK), "{state:?}");
        }
    }

    #[test]
    fn without_commits_of_its_own_nothing_is_finished() {
        let stopped = activity(Some(100), Some(200));
        for work in [
            BranchWork::default(),
            BranchWork {
                commits: 2,
                unique_commits: 2,
                merged: true,
            },
            BranchWork {
                commits: 2,
                unique_commits: 0,
                merged: false,
            },
        ] {
            assert!(
                !is_finished_work(&stopped, &AgentState::Idle, &work),
                "{work:?}"
            );
        }
    }
}
