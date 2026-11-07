//! `GhCli`, against a fake `gh` that records how it is called: nothing
//! reaches GitHub.

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;

use volee_core::delivery::{ChecksState, PrState, PullRequest};
use volee_git::{Forge, ForgeStatus, GhCli};

use crate::fixture::Sandbox;

/// A fake `gh`: logs its directory and arguments, answers `--version`,
/// answers `auth status` with `auth_code`, and prints `answer` for `pr list`.
fn fake_gh(sandbox: &Sandbox, auth_code: u8, answer: &str) -> (PathBuf, PathBuf) {
    let log = sandbox.path("gh.log");
    let answer_file = sandbox.path("answer.json");
    fs::write(&answer_file, answer).expect("write the answer of the fake gh");
    let script = format!(
        "#!/bin/sh\n\
         echo \"$(pwd) $*\" >> '{log}'\n\
         case \"$1\" in\n\
           --version) echo 'gh version 9.8.7 (2026-01-01)' ;;\n\
           auth) exit {auth_code} ;;\n\
           pr) cat '{answer}' ;;\n\
         esac\n",
        log = log.display(),
        answer = answer_file.display(),
    );
    (sandbox.script("bin/gh", &script), log)
}

#[test]
fn a_pull_request_is_looked_up_in_the_repository() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let answer = r#"[{"number":7,"url":"https://github.com/volee-org/volee/pull/7","state":"OPEN","isDraft":true,"statusCheckRollup":[{"__typename":"CheckRun","status":"IN_PROGRESS","conclusion":null}]}]"#;
    let (gh, log) = fake_gh(&sandbox, 0, answer);

    let found = GhCli::with_program(&gh)
        .pull_request(&repo, "feat/x")
        .unwrap();
    assert_eq!(
        found,
        Some(PullRequest {
            number: 7,
            url: "https://github.com/volee-org/volee/pull/7".into(),
            state: PrState::Draft,
            checks: ChecksState::Pending,
        })
    );
    let calls = fs::read_to_string(&log).unwrap();
    assert_eq!(
        calls.trim(),
        format!(
            "{} pr list --head=feat/x --state all --json number,url,state,isDraft,statusCheckRollup --limit 1",
            repo.display()
        )
    );
}

#[test]
fn a_branch_without_pull_request_has_none() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let (gh, _) = fake_gh(&sandbox, 0, "[]");
    assert_eq!(
        GhCli::with_program(&gh)
            .pull_request(&repo, "feat/x")
            .unwrap(),
        None
    );
}

#[test]
fn the_status_says_whether_gh_can_be_used() {
    let sandbox = Sandbox::new();
    let (signed_in, _) = fake_gh(&sandbox, 0, "[]");
    assert_eq!(
        GhCli::with_program(&signed_in).status(),
        ForgeStatus {
            program: "gh".into(),
            available: true,
            version: Some("9.8.7".into()),
            detail: None,
        }
    );

    let other = Sandbox::new();
    let (signed_out, _) = fake_gh(&other, 1, "[]");
    let status = GhCli::with_program(&signed_out).status();
    assert!(!status.available);
    assert_eq!(status.version.as_deref(), Some("9.8.7"));
    assert!(
        status.detail.as_deref().unwrap().contains("gh auth login"),
        "{status:?}"
    );

    let missing = GhCli::with_program(other.path("bin/no-such-gh")).status();
    assert!(!missing.available);
    assert!(
        missing.detail.as_deref().unwrap().contains("not installed"),
        "{missing:?}"
    );
}
