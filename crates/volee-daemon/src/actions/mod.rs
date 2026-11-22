//! What the person does on a finished change (v2, step 9): merge it, ask the
//! agent for a fix, comment a line, open a file in their own editor. Every
//! action goes into a journal the interface shows. See ADR 0041.

pub(crate) mod editor;
pub(crate) mod fix;
pub(crate) mod merge;

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::watch;
use volee_core::state::AgentState;
use volee_git::{Forge, Git, MergeMethod};
use volee_proto::methods::AgentCard;
use volee_rules::{EditorKind, Risk};
use volee_store::{JournalEntry, NewJournalEntry, Store, StoredComment};

use crate::agents_view::Agents;
use crate::assess::Assessments;
use crate::review::Target;
use crate::web::terminal::{Target as TerminalTarget, Terminal};

/// Where fix requests go and are confirmed, when it is not where Claude
/// Code puts it: for tests and unusual installations. The default (every
/// field `None`) is the real thing.
#[derive(Clone, Debug, Default)]
pub struct FixSources {
    /// The directory of the sessions' sockets (`/tmp/cc-socks`).
    pub socket_dir: Option<PathBuf>,
    /// Claude Code's `projects` directory, where transcripts are read to
    /// confirm a delivery.
    pub projects: Option<PathBuf>,
    /// How long a delivery waits to see the agent take the message.
    pub confirm_within: Option<Duration>,
}

/// What an action answers the interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Done {
    pub(crate) ok: bool,
    pub(crate) message: String,
    /// For a fix: the text, so the interface can offer it to copy when it
    /// was not delivered.
    pub(crate) delivered: Option<bool>,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// The actions, and the store of comments and journal they write to.
pub(crate) struct Actions {
    store: Mutex<Store>,
    git: Option<Arc<dyn Git>>,
    forge: Option<Arc<dyn Forge>>,
    /// The merge method the person set per repository (main checkout).
    methods: Vec<(PathBuf, MergeMethod)>,
    resumer: Arc<dyn fix::Resumer>,
    timing: fix::Timing,
    assessments: Arc<Assessments>,
    agents: Arc<Agents>,
    projects: Option<PathBuf>,
    /// One merge at a time: two merges of one repository must not race.
    merging: Mutex<()>,
    counter: AtomicU64,
}

impl std::fmt::Debug for Actions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Actions").finish_non_exhaustive()
    }
}

/// What `Actions::new` needs.
pub(crate) struct Parts {
    pub(crate) git: Option<Arc<dyn Git>>,
    pub(crate) forge: Option<Arc<dyn Forge>>,
    pub(crate) methods: Vec<(PathBuf, MergeMethod)>,
    pub(crate) resumer: Arc<dyn fix::Resumer>,
    pub(crate) timing: fix::Timing,
    pub(crate) assessments: Arc<Assessments>,
    pub(crate) agents: Arc<Agents>,
    pub(crate) projects: Option<PathBuf>,
}

impl Actions {
    pub(crate) fn open(parts: Parts, database: &Path) -> Result<Self, String> {
        let store = Store::open(database).map_err(|error| error.to_string())?;
        Ok(Self {
            store: Mutex::new(store),
            git: parts.git,
            forge: parts.forge,
            methods: parts.methods,
            resumer: parts.resumer,
            timing: parts.timing,
            assessments: parts.assessments,
            agents: parts.agents,
            projects: parts.projects,
            merging: Mutex::new(()),
            counter: AtomicU64::new(0),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Tells the interface something changed (the journal is part of what
    /// it shows) and reads the agents again.
    fn changed(&self) {
        self.assessments.rules().notify();
        self.agents.nudge();
    }

    fn write(
        &self,
        target: &Target,
        tip: &str,
        kind: &str,
        ok: bool,
        automatic: bool,
        message: &str,
    ) {
        let entry = NewJournalEntry {
            at: now(),
            repo: &target.repo_key(),
            branch: &target.branch,
            tip,
            kind,
            ok,
            automatic,
            message,
        };
        if let Err(error) = self.lock().journal_add(&entry) {
            tracing::warn!(%error, "cannot write the journal");
        }
        self.changed();
    }

    /// The latest entries of the journal, newest first.
    pub(crate) fn journal(&self, limit: u32) -> Vec<JournalEntry> {
        self.lock().journal(limit).unwrap_or_default()
    }

    // ---- comments -------------------------------------------------------

    /// The comments on the branch's current tip.
    pub(crate) fn comments(&self, target: &Target) -> Vec<StoredComment> {
        let Some(tip) = target.tip() else {
            return Vec::new();
        };
        self.lock()
            .comments(&target.repo_key(), &tip)
            .unwrap_or_default()
    }

    /// Adds a comment on a line of the branch's current tip.
    pub(crate) fn add_comment(
        &self,
        target: &Target,
        file: &str,
        line: u32,
        text: &str,
    ) -> Result<StoredComment, String> {
        let text = text.trim();
        if text.is_empty() || file.is_empty() || line == 0 {
            return Err("a comment needs a file, a line and a text".to_owned());
        }
        let tip = target.tip().ok_or("the branch has no tip")?;
        let at = now();
        let id = self
            .lock()
            .add_comment(&target.repo_key(), &tip, file, line, text, at)
            .map_err(|error| error.to_string())?;
        self.write(
            target,
            &tip,
            "comment",
            true,
            false,
            &format!("Commented {file}:{line}"),
        );
        Ok(StoredComment {
            id,
            file: file.to_owned(),
            line,
            text: text.to_owned(),
            created_at: at,
        })
    }

    /// Removes a comment of the branch's current tip.
    pub(crate) fn delete_comment(&self, target: &Target, id: i64) -> bool {
        let Some(tip) = target.tip() else {
            return false;
        };
        let removed = self
            .lock()
            .delete_comment(&target.repo_key(), &tip, id)
            .unwrap_or(false);
        if removed {
            self.changed();
        }
        removed
    }

    // ---- merge ----------------------------------------------------------

    fn method_of(&self, target: &Target) -> Option<MergeMethod> {
        self.methods
            .iter()
            .find(|(root, _)| *root == target.repo_root)
            .map(|(_, method)| *method)
    }

    /// Merges the agent's finished work. `rule` is `Some(reason)` when volee
    /// does it by itself because a rule said so. Blocking.
    pub(crate) fn merge(&self, agent: &AgentCard, rule: Option<&str>) -> Done {
        let automatic = rule.is_some();
        let fail = |message: String| Done {
            ok: false,
            message,
            delivered: None,
        };
        let Some(target) = Target::of(agent) else {
            return fail("this agent has no branch and base to merge".to_owned());
        };
        let Some(tip) = target.tip() else {
            return fail("the branch has no commit to merge".to_owned());
        };
        if !agent.finished_work {
            return fail(
                "this agent has no finished work to merge: it is working, or has nothing the base lacks"
                    .to_owned(),
            );
        }
        let Some(git) = self.git.clone() else {
            return fail("git is not available to this daemon".to_owned());
        };
        let _one_at_a_time = self.merging.lock().unwrap_or_else(PoisonError::into_inner);
        let outcome = merge::merge(
            git.as_ref(),
            self.forge.as_deref(),
            self.method_of(&target),
            &target,
        );
        match outcome {
            Ok(merged) => {
                let message = match rule {
                    Some(reason) => format!(
                        "Merged automatically by your rules ({reason}). {}",
                        merged.message
                    ),
                    None => merged.message,
                };
                self.write(&target, &tip, "merge", true, automatic, &message);
                Done {
                    ok: true,
                    message,
                    delivered: None,
                }
            }
            Err(why) => {
                let message = format!("Not merged, nothing changed: {why}");
                self.write(&target, &tip, "merge", false, automatic, &message);
                fail(message)
            }
        }
    }

    /// Merges what the rules allow by themselves, once per tip. Blocking.
    pub(crate) fn merge_by_rules(&self, cards: &[AgentCard]) {
        for agent in cards.iter().filter(|agent| agent.finished_work) {
            if matches!(agent.state, AgentState::Working) {
                continue;
            }
            let Some(target) = Target::of(agent) else {
                continue;
            };
            let Some(tip) = target.tip() else {
                continue;
            };
            let Some(view) = self.assessments.assess(&target) else {
                continue;
            };
            if !view.auto_merge.allowed || view.level == Risk::High {
                continue;
            }
            // Once per tip, whatever came of it: a failing merge is not
            // retried in a loop, the person sees it in the journal.
            if self
                .lock()
                .journal_has(&target.repo_key(), &tip, "merge")
                .unwrap_or(true)
            {
                continue;
            }
            let done = self.merge(agent, Some(&view.auto_merge.reason));
            tracing::info!(branch = %target.branch, ok = done.ok, "automatic merge");
        }
    }

    // ---- fix ------------------------------------------------------------

    /// Sends `request` to the agent and confirms it arrived. Blocking.
    pub(crate) fn ask_fix(
        &self,
        handle: &tokio::runtime::Handle,
        agent: &AgentCard,
        request: &str,
    ) -> Done {
        let target = Target::of(agent);
        let request = request.trim();
        if request.is_empty() {
            return Done {
                ok: false,
                message: "there is nothing to send".to_owned(),
                delivered: Some(false),
            };
        }
        let nonce = format!(
            "{:x}{:x}",
            now() & 0xffff_ffff,
            self.counter.fetch_add(1, Ordering::Relaxed)
        );
        let subject = fix::Subject {
            session_id: agent.session_id.clone(),
            pid: agent.pid,
            alive: agent.alive,
        };
        let transcript = self.projects.as_deref().and_then(|projects| {
            crate::transcript::locate(projects, &agent.cwd, &agent.session_id)
        });
        let before = transcript
            .as_ref()
            .and_then(|path| std::fs::metadata(path).ok())
            .map_or(0, |meta| meta.len());
        let was_working = matches!(agent.state, AgentState::Working);
        let (agents, id, marker) = (
            Arc::clone(&self.agents),
            agent.session_id.clone(),
            nonce.clone(),
        );
        let mut last_state_check = std::time::Instant::now();
        let mut confirmed = move || {
            if let Some(path) = &transcript
                && tail_has(path, before, &marker)
            {
                return true;
            }
            // The agent turning to working is the other sign, for an agent
            // that was not working when the message was sent.
            if !was_working && last_state_check.elapsed() >= Duration::from_secs(1) {
                last_state_check = std::time::Instant::now();
                let view = handle.block_on(agents.get(Duration::from_millis(500)));
                return view.agents.iter().any(|card| {
                    card.session_id == id && matches!(card.state, AgentState::Working)
                });
            }
            false
        };
        let outcome = fix::deliver(
            &subject,
            request,
            &nonce,
            &self.timing,
            self.resumer.as_ref(),
            &mut confirmed,
        );
        let ok = outcome.delivered();
        let said = match &outcome {
            fix::Outcome::Delivered(via, message) => {
                format!("Fix request delivered ({}). {message}", via.word())
            }
            fix::Outcome::NotDelivered(message) => format!("Fix request NOT delivered: {message}"),
        };
        if let Some(target) = &target {
            let tip = target.tip().unwrap_or_default();
            self.write(target, &tip, "fix", ok, false, &said);
        }
        Done {
            ok,
            message: said,
            delivered: Some(ok),
        }
    }

    // ---- editor ---------------------------------------------------------

    /// Opens `file` at `line` in the editor called `name`. Blocking.
    pub(crate) fn open_editor(
        &self,
        agent: &AgentCard,
        terminal: Option<&Arc<dyn Terminal>>,
        name: &str,
        file: &str,
        line: u32,
    ) -> Done {
        let fail = |message: String| Done {
            ok: false,
            message,
            delivered: None,
        };
        let Some(repo) = agent.repo.as_ref() else {
            return fail("this agent is not in a repository".to_owned());
        };
        let worktree = repo.worktree.clone().unwrap_or_else(|| agent.cwd.clone());
        let editors = self.assessments.rules().info(Some(&repo.root)).editors;
        let resolved = match editor::resolve(&editors, name, &worktree, &repo.name, file, line) {
            Ok(resolved) => resolved,
            Err(why) => return fail(why),
        };
        let opened = match resolved.kind {
            EditorKind::External => editor::run_external(&resolved),
            EditorKind::Terminal => match terminal.filter(|terminal| terminal.available()) {
                Some(terminal) => terminal.open_command(
                    &TerminalTarget {
                        pid: agent.pid,
                        cwd: agent.cwd.clone(),
                    },
                    &resolved.cwd,
                    &resolved.argv,
                ),
                None => Err("no terminal program found to open a terminal editor in".to_owned()),
            },
        };
        let (ok, message) = match opened {
            Ok(message) => (true, format!("{message}: {file}:{line}")),
            Err(why) => (false, format!("Could not open {file} in {name}: {why}")),
        };
        if let Some(target) = Target::of(agent) {
            let tip = target.tip().unwrap_or_default();
            self.write(&target, &tip, "editor", ok, false, &message);
        }
        Done {
            ok,
            message,
            delivered: None,
        }
    }

    /// The editors the person set, for a repository.
    pub(crate) fn editors(&self, root: &Path) -> Vec<volee_rules::Editor> {
        self.assessments.rules().info(Some(root)).editors
    }
}

/// Whether the transcript, past byte `from`, has `needle`.
fn tail_has(path: &Path, from: u64, needle: &str) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    if file.seek(SeekFrom::Start(from)).is_err() {
        return false;
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return false;
    }
    String::from_utf8_lossy(&bytes).contains(needle)
}

/// Merges, by the rules, what they allow, each time the agents change.
pub(crate) async fn watch_auto_merge(
    actions: Arc<Actions>,
    agents: Arc<Agents>,
    mut stop: watch::Receiver<bool>,
) {
    let mut revisions = agents.subscribe();
    // The rules changing (auto_merge turned on) is a reason to look again.
    let mut rules = actions.assessments.rules().subscribe();
    loop {
        if let Some(view) = agents.peek()
            && view.agents.iter().any(|agent| agent.finished_work)
        {
            let actions = Arc::clone(&actions);
            let cards = view.agents;
            let _ = tokio::task::spawn_blocking(move || actions.merge_by_rules(&cards)).await;
        }
        tokio::select! {
            moved = revisions.changed() => if moved.is_err() { return },
            moved = rules.changed() => if moved.is_err() { return },
            _ = stop.changed() => return,
        }
    }
}
