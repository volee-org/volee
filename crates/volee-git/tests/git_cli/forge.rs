//! `GhCli`, against a fake `gh` that records how it is called: nothing
//! reaches GitHub.

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;

use volee_core::delivery::{ChecksState, PrState, PullRequest};
use volee_git::{BranchHead, Forge, ForgeStatus, GhCli, RepoSlug};

use crate::fixture::Sandbox;

/// The tip of `feat/x` in these tests.
const TIP: &str = "6adc11fa9619748617663ded520ce631e9292269";

fn origin() -> RepoSlug {
    RepoSlug {
        owner: "volee-org".into(),
        name: "volee".into(),
    }
}

/// `feat/x` of `volee-org/volee`, at [`TIP`].
fn branch(origin: &RepoSlug) -> BranchHead<'_> {
    BranchHead {
        branch: "feat/x",
        origin: Some(origin),
        tip: TIP,
    }
}

/// A pull request as `gh pr list --json` prints it, opened from `owner/volee`
/// at `oid`.
fn listed(number: u64, state: &str, owner: &str, oid: &str) -> String {
    format!(
        r#"{{"number":{number},"url":"https://github.com/volee-org/volee/pull/{number}","state":"{state}","isDraft":false,"statusCheckRollup":[],"headRefOid":"{oid}","headRepository":{{"id":"R_1","name":"volee"}},"headRepositoryOwner":{{"id":"O_1","login":"{owner}"}}}}"#
    )
}

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
    let answer = format!(
        r#"[{{"number":7,"url":"https://github.com/volee-org/volee/pull/7","state":"OPEN","isDraft":true,"statusCheckRollup":[{{"__typename":"CheckRun","status":"IN_PROGRESS","conclusion":null}}],"headRefOid":"{TIP}","headRepository":{{"id":"R_1","name":"volee"}},"headRepositoryOwner":{{"id":"O_1","login":"volee-org"}}}}]"#
    );
    let (gh, log) = fake_gh(&sandbox, 0, &answer);

    let origin = origin();
    let found = GhCli::with_program(&gh)
        .pull_request(&repo, &branch(&origin))
        .unwrap();
    assert_eq!(
        found,
        Some(PullRequest {
            number: 7,
            url: "https://github.com/volee-org/volee/pull/7".into(),
            state: PrState::Draft,
            checks: ChecksState::Pending,
            head: Some(TIP.into()),
        })
    );
    let calls = fs::read_to_string(&log).unwrap();
    assert_eq!(
        calls.trim(),
        format!(
            "{} pr list --head=feat/x --state all --json number,url,state,isDraft,statusCheckRollup,headRefOid,headRepository,headRepositoryOwner --limit 10",
            repo.display()
        )
    );
}

#[test]
fn only_the_pull_request_of_the_branch_counts() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let old = "0dd5ed0dd5ed0dd5ed0dd5ed0dd5ed0dd5ed0dd5";
    let origin = origin();
    let look_up = |answer: &[String]| {
        let sandbox = Sandbox::new();
        let (gh, _) = fake_gh(&sandbox, 0, &format!("[{}]", answer.join(",")));
        GhCli::with_program(&gh)
            .pull_request(&repo, &branch(&origin))
            .unwrap()
            .map(|pr| pr.number)
    };

    // The branch name was used before: its old pull request was merged at
    // another commit, and says nothing about this work.
    assert_eq!(look_up(&[listed(3, "MERGED", "volee-org", old)]), None);
    // A fork has a branch with the same name, and opened a pull request.
    assert_eq!(look_up(&[listed(9, "MERGED", "someone", TIP)]), None);
    // The branch's own pull request, among the others.
    assert_eq!(
        look_up(&[
            listed(11, "OPEN", "someone", old),
            listed(10, "OPEN", "volee-org", TIP),
            listed(3, "MERGED", "volee-org", old),
        ]),
        Some(10)
    );
}

#[test]
fn a_branch_without_pull_request_has_none() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let (gh, _) = fake_gh(&sandbox, 0, "[]");
    let origin = origin();
    assert_eq!(
        GhCli::with_program(&gh)
            .pull_request(&repo, &branch(&origin))
            .unwrap(),
        None
    );
}

#[test]
fn a_branch_not_pushed_to_a_forge_is_not_looked_up() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repo("volee");
    let (gh, log) = fake_gh(
        &sandbox,
        0,
        &format!("[{}]", listed(7, "MERGED", "volee-org", TIP)),
    );
    let local = BranchHead {
        branch: "feat/x",
        origin: None,
        tip: TIP,
    };
    assert_eq!(
        GhCli::with_program(&gh)
            .pull_request(&repo, &local)
            .unwrap(),
        None
    );
    assert!(!log.exists(), "gh never ran");
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
