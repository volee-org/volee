//! The volee daemon as a [`Source`] (ADR 0003).
//!
//! The interface follows the daemon's events through the shared feed of
//! [`volee_proto::replica`], and asks the daemon for screens, diffs, take
//! overs, messages, summaries and launches with protocol calls. Starting the
//! daemon is the caller's job: this module only talks to one that runs.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::mpsc;
use volee_core::harness::HarnessKind;
use volee_core::message::Party;
use volee_core::role::Role;
use volee_core::{AgentId, Event, MessageId, ThreadId};
use volee_proto::client::{Client, ClientError};
use volee_proto::methods::{
    AgentParams, Attach, CreateMission, CreateMissionParams, Diff, DiffParams, Forget, MarkRead,
    MarkReadParams, Preview, PreviewText, SendMessage, SendParams, Spawn, SpawnParams, Status,
    StatusParams, Summarize, SummarizeParams,
};
use volee_proto::replica::{self, Feed, FeedEnd, ReplicaError, Updates};
use volee_proto::rpc;

use crate::source::{
    Changes, Door, MissionRequest, NewMission, Outgoing, PaneFocus, Reading, Source, SourceError,
    SpawnRequest, Subscription, Summary, TakeOver,
};

/// What the interface says once the daemon is gone.
const LOST: &str = "lost the connection to the volee daemon";

/// What the daemon read of an agent's screen, and what it says of it: a
/// screen itself or a replay, and the size of its terminal when it knows it.
impl From<PreviewText> for Reading {
    fn from(preview: PreviewText) -> Self {
        Self {
            text: preview.screen,
            exact: preview.exact,
            size: preview.size.map(|size| (size.cols, size.rows)),
        }
    }
}

/// How many events wait for the interface before the feed waits for it.
const BACKLOG: usize = 1_024;

/// A [`Source`] backed by the volee daemon: the agents of the whole machine.
///
/// ```no_run
/// use volee_proto::client::Client;
/// use volee_proto::paths::Paths;
/// use volee_tui::{DaemonSource, Options};
///
/// # async fn interface() -> Result<(), Box<dyn std::error::Error>> {
/// let client = Client::connect(&Paths::from_env()?.socket).await?;
/// let source = DaemonSource::connect(client).await?;
/// volee_tui::run(source, Options::default()).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct DaemonSource {
    client: Arc<Client>,
    feed: Mutex<Option<Feed>>,
    /// Whether the daemon found a Claude Code it can use.
    claude_code_available: bool,
}

impl DaemonSource {
    /// Subscribes `client` to the daemon's events. The client must be fresh
    /// (its notifications not taken) and talk to a daemon of the same
    /// protocol version, which the command line checks when it starts or
    /// finds the daemon.
    ///
    /// The subscription is interactive: while the interface is open, the
    /// daemon looks for new sessions every few seconds, and it looks once
    /// right away so the interface opens on fresh data. The daemon's status
    /// says first whether it found a Claude Code it can use.
    pub async fn connect(mut client: Client) -> Result<Self, SourceError> {
        let status = client
            .call::<Status>(StatusParams {
                protocol: volee_proto::PROTOCOL_VERSION,
            })
            .await
            .map_err(|error| failure(error, "cannot ask the volee daemon for its status"))?;
        let claude_code_available = status
            .harnesses
            .iter()
            .any(|harness| harness.kind == HarnessKind::ClaudeCode && harness.available);
        let feed = replica::subscribe_interactive(&mut client)
            .await
            .map_err(cannot_follow)?;
        Ok(Self {
            client: Arc::new(client),
            feed: Mutex::new(Some(feed)),
            claude_code_available,
        })
    }
}

/// What the interface says when it cannot subscribe to the daemon.
fn cannot_follow(error: ReplicaError) -> SourceError {
    match error {
        ReplicaError::Client(error) => failure(error, "cannot follow the volee daemon"),
        other => SourceError::Failed(other.to_string()),
    }
}

impl DaemonSource {
    /// Hands the feed to the interface: the backlog at once, the updates
    /// through a task.
    fn follow(&self) -> Result<Subscription, SourceError> {
        let feed = self
            .feed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .ok_or_else(|| SourceError::Failed("the interface subscribes only once".to_owned()))?;
        let (sender, updates) = mpsc::channel(BACKLOG);
        tokio::spawn(forward(Arc::clone(&self.client), feed.updates, sender));
        Ok(Subscription {
            snapshot: feed.backlog,
            updates,
            claude_code_available: self.claude_code_available,
        })
    }
}

impl Source for DaemonSource {
    /// Must be called inside a Tokio runtime: a task forwards the feed.
    fn subscribe(&self) -> impl Future<Output = Result<Subscription, SourceError>> + Send {
        std::future::ready(self.follow())
    }

    async fn preview(&self, agent: AgentId) -> Result<Reading, SourceError> {
        self.client
            .call::<Preview>(AgentParams { agent })
            .await
            .map(Reading::from)
            .map_err(|error| failure(error, "this volee daemon cannot show agent screens yet"))
    }

    async fn diff(&self, agent: AgentId, full: bool) -> Result<Changes, SourceError> {
        self.client
            .call::<Diff>(DiffParams { agent, full })
            .await
            .map(|diff| Changes {
                text: diff.text,
                truncated: diff.truncated,
            })
            .map_err(|error| failure(error, "this volee daemon cannot show changes yet"))
    }

    async fn send_message(&self, message: Outgoing) -> Result<(), SourceError> {
        let params = SendParams {
            from: Party::Human,
            to: vec![Party::Agent(message.to)],
            kind: message.kind,
            body: message.body,
            refs: vec![],
            in_reply_to: message.in_reply_to,
            thread: message.thread,
        };
        self.client
            .call::<SendMessage>(params)
            .await
            .map(|_| ())
            .map_err(|error| failure(error, "this volee daemon cannot send messages"))
    }

    async fn attach(&self, agent: AgentId) -> Result<TakeOver, SourceError> {
        let command = self
            .client
            .call::<Attach>(AgentParams { agent })
            .await
            .map_err(|error| failure(error, "this volee daemon cannot take over agents yet"))?;
        Ok(TakeOver {
            program: command.program,
            args: command.args,
            cwd: command.cwd.map(PathBuf::from),
            env_remove: command.env_remove,
            hint: Some(command.hint).filter(|hint| !hint.trim().is_empty()),
            focus: command.focus.map(|focus| PaneFocus {
                multiplexer: focus.multiplexer,
                session: focus.session,
                steps: focus.steps,
                attach: focus.attach,
            }),
        })
    }

    async fn open(&self, agent: AgentId, size: (u16, u16)) -> Result<Door, SourceError> {
        let opened = self
            .client
            .call::<volee_proto::methods::Open>(volee_proto::methods::OpenParams {
                agent,
                cols: size.0,
                rows: size.1,
            })
            .await
            .map_err(|error| failure(error, "this volee daemon cannot open agents yet"))?;
        Ok(match opened.socket {
            Some(socket) => Door::Host {
                socket,
                view: opened.view,
            },
            None => Door::Elsewhere,
        })
    }

    async fn mark_read(&self, message: MessageId) -> Result<(), SourceError> {
        self.client
            .call::<MarkRead>(MarkReadParams {
                message,
                by: Party::Human,
            })
            .await
            .map(|_| ())
            .map_err(|error| failure(error, "this volee daemon cannot mark messages read"))
    }

    async fn summarize(&self, thread: ThreadId) -> Result<Summary, SourceError> {
        self.client
            .call::<Summarize>(SummarizeParams { thread })
            .await
            .map(|summary| Summary {
                text: summary.summary,
                messages: summary.messages,
                model: summary.model,
            })
            .map_err(|error| failure(error, "this volee daemon cannot summarize threads yet"))
    }

    async fn forget(&self, agent: AgentId) -> Result<(), SourceError> {
        self.client
            .call::<Forget>(AgentParams { agent })
            .await
            .map(|_| ())
            .map_err(|error| failure(error, "this volee daemon cannot forget agents"))
    }

    async fn adopt(&self, agent: AgentId) -> Result<(), SourceError> {
        self.client
            .call::<volee_proto::methods::Adopt>(AgentParams { agent })
            .await
            .map(|_| ())
            .map_err(|error| failure(error, "this volee daemon cannot adopt agents"))
    }

    async fn spawn(&self, request: SpawnRequest) -> Result<AgentId, SourceError> {
        let params = SpawnParams {
            repo: request.repo,
            branch: request.branch,
            base: None,
            task: request.task,
            // An agent the person launches answers to the person.
            role: Role::Lead,
            parent: None,
            mission: None,
            model: request.model,
            name: None,
            harness: request.harness,
        };
        self.client
            .call::<Spawn>(params)
            .await
            .map(|spawned| spawned.agent)
            .map_err(|error| failure(error, "this volee daemon cannot launch agents"))
    }

    async fn create_mission(&self, request: MissionRequest) -> Result<NewMission, SourceError> {
        let params = CreateMissionParams {
            title: request.title,
            kind: request.kind,
            goal: request.goal,
            repos: request.repos,
            requested_by: None,
        };
        self.client
            .call::<CreateMission>(params)
            .await
            .map(|created| NewMission {
                mission: created.mission,
                coordinator: created.coordinator,
            })
            .map_err(|error| failure(error, "this volee daemon cannot create missions"))
    }
}

/// Passes the feed's events to the interface until either side stops. A
/// subscription the daemon dropped for falling behind is picked up again
/// where it stopped.
async fn forward(
    client: Arc<Client>,
    mut updates: Updates,
    sender: mpsc::Sender<Result<Event, SourceError>>,
) {
    loop {
        let next = tokio::select! {
            () = sender.closed() => return,
            next = updates.next() => next,
        };
        let item = match next {
            Ok(event) => Ok(event),
            Err(FeedEnd::Lagged { skipped }) => {
                tracing::warn!(
                    skipped,
                    "the interface fell behind the daemon; subscribing again"
                );
                match updates.resume(&client).await {
                    Ok(()) => continue,
                    Err(error) => Err(failure(error, LOST)),
                }
            }
            Err(FeedEnd::Closed) => Err(SourceError::Disconnected(LOST.to_owned())),
            Err(end) => Err(SourceError::Failed(end.to_string())),
        };
        let last = item.is_err();
        if sender.send(item).await.is_err() || last {
            return;
        }
    }
}

/// The interface's view of a failed call. `missing` says what the daemon
/// cannot do when it does not know the method: an older daemon, or a
/// method another version adds.
fn failure(error: ClientError, missing: &str) -> SourceError {
    match error {
        ClientError::Rpc(error) => match error.code {
            rpc::METHOD_NOT_FOUND => SourceError::Unsupported(missing.to_owned()),
            // A rule says no, such as a session in a terminal of its own:
            // asking again will not help. Anything else (a program missing,
            // a session not listed yet) may pass later.
            rpc::REFUSED => SourceError::Unsupported(error.message),
            _ => SourceError::Failed(error.message),
        },
        ClientError::Connect { .. } | ClientError::Io(_) | ClientError::Closed => {
            SourceError::Disconnected(LOST.to_owned())
        }
        ClientError::Protocol(message) => SourceError::Failed(message),
    }
}

#[cfg(test)]
mod tests {
    use volee_proto::methods::ScreenSize;
    use volee_proto::rpc::RpcError;

    use super::*;

    #[test]
    fn what_the_daemon_says_of_a_screen_reaches_the_interface() {
        // The snapshot of a volee host, with the size of its terminal.
        let held = PreviewText {
            screen: "held".into(),
            exact: true,
            size: Some(ScreenSize {
                cols: 120,
                rows: 60,
            }),
        };
        assert_eq!(
            Reading::from(held),
            Reading::screen("held", Some((120, 60)))
        );
        // The capture of a multiplexer's pane: a screen, no size.
        let captured = PreviewText {
            screen: "captured".into(),
            exact: true,
            size: None,
        };
        assert_eq!(Reading::from(captured), Reading::screen("captured", None));
        // The replay of a session's output, and all an older daemon says.
        let replayed = PreviewText {
            screen: "replayed".into(),
            exact: false,
            size: None,
        };
        assert_eq!(Reading::from(replayed), Reading::replay("replayed"));
    }

    #[test]
    fn missing_methods_and_refusals_are_unsupported() {
        let rpc = |code, message: &str| ClientError::Rpc(RpcError::new(code, message));
        assert_eq!(
            failure(
                rpc(rpc::METHOD_NOT_FOUND, "unknown method"),
                "no screens yet"
            ),
            SourceError::Unsupported("no screens yet".into())
        );
        assert_eq!(
            failure(rpc(rpc::REFUSED, "api runs in a terminal of its own"), "x"),
            SourceError::Unsupported("api runs in a terminal of its own".into())
        );
        assert_eq!(
            failure(rpc(rpc::UNAVAILABLE, "try again in a moment"), "x"),
            SourceError::Failed("try again in a moment".into())
        );
        assert_eq!(
            failure(rpc(rpc::NOT_FOUND, "no agent ag_1"), "x"),
            SourceError::Failed("no agent ag_1".into())
        );
        assert_eq!(
            failure(ClientError::Closed, "x"),
            SourceError::Disconnected(LOST.into())
        );
    }
}
