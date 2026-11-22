//! The Lua rules inside the daemon (v2, step 8).
//!
//! Lua never runs on the async loop. One worker thread owns the loaded rules
//! (one set per repository, plus the personal file alone) and answers
//! requests over a channel; a caller waits for the answer for a limited time
//! and treats a late answer as the safest outcome (high risk, no automatic
//! merge, every layer). A worker that overran is abandoned and replaced, so
//! one pathological pattern cannot stall the daemon.
//!
//! The rules files are read again when they change: every request carries
//! the files' modification stamp, and a poller notices a change while nobody
//! asks and tells the interface. Evaluations are cached per repository and
//! per change tip, for as long as the stamp holds.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use tokio::sync::watch;
use volee_rules::{
    Change, Evaluation, FinishedFacts, Layer, MergeDecision, PERSONAL_RULES_FILE, REPO_RULES_PATH,
    ReviewPlan, Risk, RiskAssessment, RuleError, Rules, Settings,
};

/// How long an evaluation may take before its answer is the safest one.
const EVALUATE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the finished-work rule may take; it runs for every agent.
const FINISHED_TIMEOUT: Duration = Duration::from_secs(2);
/// How often the rules files are looked at while nobody asks.
const POLL: Duration = Duration::from_secs(2);

/// What a rules file looked like: its modification time and size. A file
/// that is not there is `None`.
type FileStamp = Option<(SystemTime, u64)>;
/// The repository's file and the personal file.
type Stamp = [FileStamp; 2];

fn file_stamp(path: &Path) -> FileStamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn stamp_of(root: Option<&Path>, config_dir: &Path) -> Stamp {
    [
        root.and_then(|root| file_stamp(&root.join(REPO_RULES_PATH))),
        file_stamp(&config_dir.join(PERSONAL_RULES_FILE)),
    ]
}

/// An evaluation kept for a tip, and the stamp it holds for.
type Cached = (Stamp, Arc<Report>);

/// All the answers about one change, and where they came from.
#[derive(Clone, Debug)]
pub(crate) struct Report {
    pub(crate) evaluation: Evaluation,
}

/// What a request for the state of the rules of one repository answers.
#[derive(Clone, Debug, Default)]
pub(crate) struct Info {
    /// How long a held question waits.
    pub(crate) held_question_timeout: Duration,
    /// Why the rules fell back to the safest outcome.
    pub(crate) errors: Vec<RuleError>,
    /// Probable typos, which are not errors.
    pub(crate) warnings: Vec<String>,
    /// The repository's file is present and not trusted.
    pub(crate) restricted: bool,
    /// The person's editors (`editors` in the rules).
    pub(crate) editors: Vec<volee_rules::Editor>,
}

enum Job {
    Evaluate {
        root: PathBuf,
        stamp: Stamp,
        change: Change,
        reply: SyncSender<Report>,
    },
    Layers {
        root: PathBuf,
        stamp: Stamp,
        change: Change,
        risk: RiskAssessment,
        reply: SyncSender<Vec<Layer>>,
    },
    Finished {
        root: Option<PathBuf>,
        stamp: Stamp,
        facts: FinishedFacts,
        reply: SyncSender<bool>,
    },
    Info {
        root: Option<PathBuf>,
        stamp: Stamp,
        reply: SyncSender<Info>,
    },
}

/// The worker's state: the rules it has loaded, by repository.
struct Loaded {
    config_dir: PathBuf,
    by_root: HashMap<Option<PathBuf>, (Stamp, Rules)>,
}

impl Loaded {
    fn rules(&mut self, root: Option<&Path>, stamp: Stamp) -> &Rules {
        let key = root.map(Path::to_path_buf);
        let stale = self
            .by_root
            .get(&key)
            .is_none_or(|(loaded, _)| *loaded != stamp);
        if stale {
            let rules = Rules::load(root, Some(&self.config_dir));
            tracing::debug!(
                root = ?root,
                errors = rules.errors().len(),
                "the rules were loaded"
            );
            self.by_root.insert(key.clone(), (stamp, rules));
        }
        &self.by_root[&key].1
    }

    fn handle(&mut self, job: Job) {
        // A caller that gave up has dropped its receiver: the send fails and
        // nothing else is lost.
        match job {
            Job::Evaluate {
                root,
                stamp,
                change,
                reply,
            } => {
                let evaluation = self.rules(Some(&root), stamp).evaluate(&change);
                let _ = reply.send(Report { evaluation });
            }
            Job::Layers {
                root,
                stamp,
                change,
                risk,
                reply,
            } => {
                let layers = self.rules(Some(&root), stamp).layers(&change, &risk).value;
                let _ = reply.send(layers);
            }
            Job::Finished {
                root,
                stamp,
                facts,
                reply,
            } => {
                let finished = self.rules(root.as_deref(), stamp).finished(&facts).value;
                let _ = reply.send(finished);
            }
            Job::Info { root, stamp, reply } => {
                let rules = self.rules(root.as_deref(), stamp);
                let _ = reply.send(Info {
                    held_question_timeout: rules.settings().held_question_timeout(),
                    errors: rules.errors().to_vec(),
                    warnings: rules.warnings().to_vec(),
                    restricted: rules.is_restricted(),
                    editors: rules.editors().to_vec(),
                });
            }
        }
    }
}

/// The sender of the current worker, and which worker it is.
struct Worker {
    jobs: mpsc::Sender<Job>,
    generation: u64,
}

/// The rules of the daemon: a worker thread, a cache and a change notifier.
pub(crate) struct RulesService {
    config_dir: PathBuf,
    evaluate_timeout: Duration,
    finished_timeout: Duration,
    worker: Mutex<Worker>,
    /// Evaluations by repository root and change tip, valid while the stamp
    /// of the rules files holds.
    cache: Mutex<HashMap<(PathBuf, String), Cached>>,
    /// The repositories asked about, with the stamp last seen.
    known: Mutex<HashMap<Option<PathBuf>, Stamp>>,
    changes: watch::Sender<u64>,
}

impl std::fmt::Debug for RulesService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RulesService").finish_non_exhaustive()
    }
}

fn spawn_worker(config_dir: &Path) -> mpsc::Sender<Job> {
    let (jobs, queue) = mpsc::channel::<Job>();
    let mut loaded = Loaded {
        config_dir: config_dir.to_path_buf(),
        by_root: HashMap::new(),
    };
    let built = std::thread::Builder::new()
        .name("volee-rules".to_owned())
        .spawn(move || {
            while let Ok(job) = queue.recv() {
                loaded.handle(job);
            }
        });
    if let Err(error) = built {
        tracing::error!(%error, "cannot start the rules thread");
    }
    jobs
}

/// The answer when the rules cannot answer in time: the safest one.
fn safest(why: &str) -> Report {
    Report {
        evaluation: Evaluation {
            risk: RiskAssessment {
                level: Risk::High,
                reasons: vec![why.to_owned()],
            },
            layers: Layer::ALL.to_vec(),
            review: ReviewPlan {
                model: Settings::default().review.model,
                reviewers: 2,
            },
            merge: MergeDecision {
                allowed: false,
                reason: why.to_owned(),
            },
            errors: vec![RuleError {
                file: None,
                line: None,
                function: None,
                message: why.to_owned(),
            }],
            notes: Vec::new(),
        },
    }
}

impl RulesService {
    pub(crate) fn new(config_dir: PathBuf) -> Arc<Self> {
        Self::with_timeouts(config_dir, EVALUATE_TIMEOUT, FINISHED_TIMEOUT)
    }

    pub(crate) fn with_timeouts(
        config_dir: PathBuf,
        evaluate_timeout: Duration,
        finished_timeout: Duration,
    ) -> Arc<Self> {
        let jobs = spawn_worker(&config_dir);
        Arc::new(Self {
            config_dir,
            evaluate_timeout,
            finished_timeout,
            worker: Mutex::new(Worker {
                jobs,
                generation: 0,
            }),
            cache: Mutex::new(HashMap::new()),
            known: Mutex::new(HashMap::new()),
            changes: watch::channel(0).0,
        })
    }

    /// A receiver that wakes each time the rules change on disk or the
    /// person changed a risk by hand.
    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    /// Tells the subscribers that what the rules say may have changed.
    pub(crate) fn notify(&self) {
        self.changes.send_modify(|count| *count += 1);
    }

    fn stamp(&self, root: Option<&Path>) -> Stamp {
        let stamp = stamp_of(root, &self.config_dir);
        self.known
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(root.map(Path::to_path_buf), stamp);
        stamp
    }

    /// Sends a job and waits for its answer, at most `timeout`. `None` means
    /// the answer is late (or the worker is gone): the worker is replaced.
    fn ask<T>(&self, timeout: Duration, job: impl FnOnce(SyncSender<T>) -> Job) -> Option<T> {
        let (reply, answer) = mpsc::sync_channel(1);
        let generation = {
            let worker = self.worker.lock().unwrap_or_else(PoisonError::into_inner);
            if worker.jobs.send(job(reply)).is_err() {
                drop(worker);
                self.replace(None);
                return None;
            }
            worker.generation
        };
        if let Ok(value) = answer.recv_timeout(timeout) {
            return Some(value);
        }
        tracing::warn!(
            timeout_ms = timeout.as_millis(),
            "the rules did not answer in time: the safest outcome is used"
        );
        self.replace(Some(generation));
        None
    }

    /// Replaces the worker, unless another caller already did.
    fn replace(&self, seen: Option<u64>) {
        let mut worker = self.worker.lock().unwrap_or_else(PoisonError::into_inner);
        if seen.is_none_or(|seen| seen == worker.generation) {
            worker.jobs = spawn_worker(&self.config_dir);
            worker.generation += 1;
        }
    }

    /// Risk, layers, review plan and merge decision for the change at `tip`
    /// of the repository at `root`. Cached per repository and tip while the
    /// rules files stay the same. Blocking.
    pub(crate) fn evaluate(&self, root: &Path, tip: &str, change: &Change) -> Arc<Report> {
        let stamp = self.stamp(Some(root));
        let key = (root.to_path_buf(), tip.to_owned());
        if let Some((cached, report)) = self
            .cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            && *cached == stamp
        {
            return Arc::clone(report);
        }
        let report = self
            .ask(self.evaluate_timeout, |reply| Job::Evaluate {
                root: root.to_path_buf(),
                stamp,
                change: change.clone(),
                reply,
            })
            .unwrap_or_else(|| safest("the rules took too long to answer"));
        let late = report
            .evaluation
            .errors
            .iter()
            .any(|error| error.message == "the rules took too long to answer");
        let report = Arc::new(report);
        if !late {
            self.cache
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(key, (stamp, Arc::clone(&report)));
        }
        report
    }

    /// The layers for `change` at `risk`, which may not be the risk the
    /// rules computed (a person can set it). Every layer when the rules do
    /// not answer in time. Blocking.
    pub(crate) fn layers(&self, root: &Path, change: &Change, risk: &RiskAssessment) -> Vec<Layer> {
        let stamp = self.stamp(Some(root));
        self.ask(self.evaluate_timeout, |reply| Job::Layers {
            root: root.to_path_buf(),
            stamp,
            change: change.clone(),
            risk: risk.clone(),
            reply,
        })
        .unwrap_or_else(|| Layer::ALL.to_vec())
    }

    /// Whether an agent has finished work, by the `finished` rule or the
    /// built-in one (also the answer when the rules are late). Blocking.
    pub(crate) fn finished(&self, root: Option<&Path>, facts: &FinishedFacts) -> bool {
        let stamp = self.stamp(root);
        self.ask(self.finished_timeout, |reply| Job::Finished {
            root: root.map(Path::to_path_buf),
            stamp,
            facts: facts.clone(),
            reply,
        })
        .unwrap_or_else(|| builtin_finished(facts))
    }

    /// The state of the rules of one repository (or of the personal file
    /// alone): the held question delay and what is wrong. Blocking.
    pub(crate) fn info(&self, root: Option<&Path>) -> Info {
        let stamp = self.stamp(root);
        self.ask(self.evaluate_timeout, |reply| Job::Info {
            root: root.map(Path::to_path_buf),
            stamp,
            reply,
        })
        .unwrap_or_else(|| Info {
            held_question_timeout: Settings::default().held_question_timeout(),
            errors: vec![RuleError {
                file: None,
                line: None,
                function: None,
                message: "the rules took too long to answer".to_owned(),
            }],
            warnings: Vec::new(),
            restricted: false,
            editors: Vec::new(),
        })
    }

    /// Looks at the rules files of every repository asked about, until
    /// `stop` says so, and tells the subscribers when one changed.
    pub(crate) async fn watch(self: Arc<Self>, mut stop: watch::Receiver<bool>) {
        loop {
            tokio::select! {
                () = tokio::time::sleep(POLL) => {}
                _ = stop.changed() => return,
            }
            let service = Arc::clone(&self);
            if tokio::task::spawn_blocking(move || service.look())
                .await
                .unwrap_or(false)
            {
                self.notify();
            }
        }
    }

    /// Whether a known repository's rules files changed since last seen.
    fn look(&self) -> bool {
        let mut known = self.known.lock().unwrap_or_else(PoisonError::into_inner);
        let mut changed = false;
        for (root, seen) in known.iter_mut() {
            let now = stamp_of(root.as_deref(), &self.config_dir);
            if now != *seen {
                *seen = now;
                changed = true;
            }
        }
        changed
    }
}

/// The built-in finished-work rule, as `volee_core::finished` states it.
fn builtin_finished(facts: &FinishedFacts) -> bool {
    !facts.busy
        && facts.stopped_last
        && !facts.merged
        && facts.commits > 0
        && facts.unique_commits > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs() -> (tempfile::TempDir, tempfile::TempDir) {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".volee")).unwrap();
        (repo, tempfile::tempdir().unwrap())
    }

    fn change(path: &str) -> Change {
        Change::example(path, 3, 1)
    }

    fn write(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn the_same_change_is_evaluated_once_and_again_after_the_rules_change() {
        let (repo, config) = dirs();
        let rules_file = repo.path().join(".volee/rules.lua");
        write(
            &rules_file,
            "return { sensitive_paths = { 'src/auth/**' } }",
        );
        let service = RulesService::new(config.path().to_path_buf());
        let sensitive = change("src/auth/login.rs");
        let first = service.evaluate(repo.path(), "tip1", &sensitive);
        assert_eq!(first.evaluation.risk.level, Risk::High);
        assert!(Arc::ptr_eq(
            &first,
            &service.evaluate(repo.path(), "tip1", &sensitive)
        ));
        // Another tip is judged again.
        assert!(!Arc::ptr_eq(
            &first,
            &service.evaluate(repo.path(), "tip2", &sensitive)
        ));
        // The file changes (its size does): the same tip is judged again.
        write(
            &rules_file,
            "return { sensitive_paths = { 'src/other/**' } }  ",
        );
        let after = service.evaluate(repo.path(), "tip1", &sensitive);
        assert_eq!(after.evaluation.risk.level, Risk::Low);
    }

    #[test]
    fn the_poller_notices_a_change_nobody_asked_about() {
        let (repo, config) = dirs();
        let rules_file = repo.path().join(".volee/rules.lua");
        write(&rules_file, "return {}");
        let service = RulesService::new(config.path().to_path_buf());
        service.evaluate(repo.path(), "t", &change("a.rs"));
        assert!(!service.look(), "nothing changed yet");
        write(&rules_file, "return { auto_merge = true }");
        assert!(service.look());
        assert!(!service.look(), "told once");
        write(&config.path().join("rules.lua"), "return {}");
        assert!(
            service.look(),
            "the personal file counts for every repository"
        );
    }

    #[test]
    fn a_broken_file_gives_the_safest_answer_and_says_where() {
        let (repo, config) = dirs();
        write(
            &repo.path().join(".volee/rules.lua"),
            "return {\n  risk = ,\n}",
        );
        let service = RulesService::new(config.path().to_path_buf());
        let report = service.evaluate(repo.path(), "t", &change("README.md"));
        assert_eq!(report.evaluation.risk.level, Risk::High);
        assert!(!report.evaluation.merge.allowed);
        let error = report.evaluation.errors.first().unwrap();
        assert!(
            error.file.as_deref().unwrap().ends_with("rules.lua"),
            "{error}"
        );
        assert_eq!(error.line, Some(2));
        let info = service.info(Some(repo.path()));
        assert_eq!(info.errors.len(), 1);
        // Fixed: back to the computed answer.
        write(
            &repo.path().join(".volee/rules.lua"),
            "return { auto_merge = false } ",
        );
        let fixed = service.evaluate(repo.path(), "t", &change("README.md"));
        assert_eq!(fixed.evaluation.risk.level, Risk::Low);
        assert_eq!(service.info(Some(repo.path())).errors, []);
    }

    #[test]
    fn a_late_answer_is_the_safest_one_and_the_service_goes_on() {
        let (repo, config) = dirs();
        // A rule that takes a while but stays within its budget.
        write(
            &repo.path().join(".volee/rules.lua"),
            "return { risk = function() local n = 0 for i = 1, 3000000 do n = n + i end return 'low' end }",
        );
        let service = RulesService::with_timeouts(
            config.path().to_path_buf(),
            Duration::from_millis(1),
            Duration::from_millis(1),
        );
        let late = service.evaluate(repo.path(), "t", &change("README.md"));
        assert_eq!(late.evaluation.risk.level, Risk::High);
        assert!(!late.evaluation.merge.allowed);
        assert_eq!(late.evaluation.layers, Layer::ALL);
        assert!(late.evaluation.errors[0].message.contains("too long"));
        // The late answer was not cached, and a fresh worker answers the
        // next question: with time to spare it is the rule's answer.
        let patient = RulesService::new(config.path().to_path_buf());
        let on_time = patient.evaluate(repo.path(), "t", &change("README.md"));
        assert_eq!(on_time.evaluation.risk.level, Risk::Low);
        // The finished rule falls back to the built-in one when late.
        let facts = FinishedFacts {
            stopped_last: true,
            busy: false,
            commits: 1,
            unique_commits: 1,
            merged: false,
        };
        assert!(service.finished(Some(repo.path()), &facts));
    }

    #[test]
    fn the_finished_rule_and_the_held_question_delay_come_from_the_rules() {
        let (repo, config) = dirs();
        write(
            &repo.path().join(".volee/rules.lua"),
            "return { held_question_timeout = '20m', finished = function(f) return f.commits >= 3 end }",
        );
        let service = RulesService::new(config.path().to_path_buf());
        assert_eq!(
            service.info(Some(repo.path())).held_question_timeout,
            Duration::from_secs(1200)
        );
        let mut facts = FinishedFacts {
            stopped_last: true,
            busy: false,
            commits: 1,
            unique_commits: 1,
            merged: false,
        };
        assert!(
            !service.finished(Some(repo.path()), &facts),
            "the rule asks for 3 commits"
        );
        facts.commits = 3;
        assert!(service.finished(Some(repo.path()), &facts));
        // Without a repository, the defaults.
        assert_eq!(
            service.info(None).held_question_timeout,
            Duration::from_secs(600)
        );
    }
}
