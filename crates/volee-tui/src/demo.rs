//! A simulated set of agents, for `volee demo`, screenshots and tests.
//!
//! The scenario is a fictional team: a lead agent, a change mission
//! "Invoicing" with a coordinator and three workers (one waits for a
//! permission, one asked a question, one opened pull request #42), an
//! analysis mission with two read-only workers (one runs two subagents), and
//! two sessions volee did not launch. States change every couple of seconds
//! and the whole story loops. Everything derives from the start time, so a
//! fixed clock gives the same screen on every run.

mod scenario;
mod screens;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::mpsc;
use ulid::Ulid;
use volee_core::event::{Event, EventKind};
use volee_core::message::{Message, Party, Receipt, Thread};
use volee_core::time::Timestamp;
use volee_core::{AgentId, MessageId, Registry, ThreadId};

use crate::clock::Clock;
use crate::source::{Changes, Outgoing, Source, SourceError, Subscription, TakeOver};

/// A [`Source`] with a simulated team of agents, for `volee demo`,
/// screenshots and tests.
///
/// A lead agent runs a change mission, "Invoicing", whose coordinator has
/// three workers: one waits for a permission to run `bun run db:migrate`, one
/// asked a question, one opened pull request #42 and its checks pass. An
/// analysis mission has two read-only workers, one of them with two
/// subagents, and two sessions volee did not launch run on their own (one in
/// the background, one in a terminal). The agents send each other a
/// contract, a question and a report, and states change every couple of
/// seconds in a loop.
///
/// Everything derives from the start time: with a fixed clock, every run
/// shows the same screen.
#[derive(Debug, Clone)]
pub struct DemoSource {
    start: Timestamp,
    clock: Arc<dyn Clock>,
    state: Arc<Mutex<Outbox>>,
}

/// Where the demo sends its events.
type Updates = mpsc::Sender<Result<Event, SourceError>>;

/// What the demo needs to post the person's messages.
#[derive(Debug, Default)]
struct Outbox {
    sender: Option<Updates>,
    threads: Vec<(AgentId, ThreadId)>,
    sent: u128,
}

impl DemoSource {
    /// A demo that starts now, according to `clock`.
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        let start = clock.now();
        Self::starting_at(start, clock)
    }

    /// A demo whose scenario starts at `start`. Events before it are the
    /// history; events after it arrive as the clock reaches them.
    pub fn starting_at(start: Timestamp, clock: Arc<dyn Clock>) -> Self {
        Self {
            start,
            clock,
            state: Arc::new(Mutex::new(Outbox::default())),
        }
    }

    /// When the scenario starts.
    pub fn start(&self) -> Timestamp {
        self.start
    }

    /// Every event of the scenario up to `until`, oldest first.
    pub fn events_until(&self, until: Timestamp) -> Vec<Event> {
        scenario::events_until(self.start, until)
    }

    /// The screen of an agent at `at`, as raw terminal output.
    pub fn screen_at(&self, agent: AgentId, at: Timestamp) -> Result<String, SourceError> {
        let registry = Registry::from_events(&self.events_until(at))
            .map_err(|error| SourceError::Failed(error.to_string()))?;
        let record = registry
            .agent(agent)
            .ok_or(SourceError::UnknownAgent(agent))?;
        Ok(screens::screen(record))
    }

    fn outbox(&self) -> std::sync::MutexGuard<'_, Outbox> {
        // The outbox holds plain data, so a panic elsewhere cannot leave it
        // inconsistent.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Source for DemoSource {
    /// Must be called inside a Tokio runtime: the scenario's later events are
    /// sent by a task, in time.
    fn subscribe(&self) -> impl Future<Output = Result<Subscription, SourceError>> + Send {
        let now = self.clock.now();
        let snapshot = self.events_until(now);
        let (sender, updates) = mpsc::channel(256);
        self.outbox().sender = Some(sender.clone());
        let start = self.start;
        tokio::spawn(async move {
            let origin = tokio::time::Instant::now();
            for event in scenario::beats(start).skip_while(|event| event.at <= now) {
                let wait = Duration::from_millis(now.millis_until(event.at));
                tokio::time::sleep_until(origin + wait).await;
                if sender.send(Ok(event)).await.is_err() {
                    break;
                }
            }
        });
        std::future::ready(Ok(Subscription { snapshot, updates }))
    }

    fn preview(&self, agent: AgentId) -> impl Future<Output = Result<String, SourceError>> + Send {
        std::future::ready(self.screen_at(agent, self.clock.now()))
    }

    fn diff(
        &self,
        agent: AgentId,
        full: bool,
    ) -> impl Future<Output = Result<Changes, SourceError>> + Send {
        let text = self.diff_at(agent, full, self.clock.now());
        std::future::ready(text.map(|text| Changes {
            text,
            truncated: false,
        }))
    }

    async fn send_message(&self, message: Outgoing) -> Result<(), SourceError> {
        let to = message.to;
        let member = scenario::member_of(self.start, to).ok_or(SourceError::UnknownAgent(to))?;
        let now = self.clock.now();
        let (sender, events) = self.message_events(member, message, now)?;
        for event in events {
            sender
                .send(Ok(event))
                .await
                .map_err(|_| SourceError::Disconnected("the interface stopped".to_owned()))?;
        }
        Ok(())
    }

    fn attach(&self, agent: AgentId) -> impl Future<Output = Result<TakeOver, SourceError>> + Send {
        std::future::ready(self.take_over(agent))
    }
}

impl DemoSource {
    /// A harmless stand-in for `claude attach`: it says what would happen
    /// and waits for Enter, so the demo shows the round trip. Only the
    /// background sessions can be taken over, as with Claude Code.
    fn take_over(&self, agent: AgentId) -> Result<TakeOver, SourceError> {
        let member =
            scenario::member_of(self.start, agent).ok_or(SourceError::UnknownAgent(agent))?;
        let cast = &scenario::CAST[member];
        if cast.role == volee_core::role::Role::Subagent {
            return Err(SourceError::Unsupported(
                "it runs inside its parent's session".to_owned(),
            ));
        }
        if !cast.background {
            return Err(SourceError::Unsupported(
                "it runs in another terminal; switch to it there".to_owned(),
            ));
        }
        let script = "printf '\\n  volee demo: here volee would run claude attach %s\\n  \
                      and show the session of %s in this terminal.\\n\\n  \
                      Press Enter to come back to volee. ' \"$1\" \"$2\"; read -r _";
        Ok(TakeOver {
            program: "sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                script.to_owned(),
                "volee-demo".to_owned(),
                scenario::session_of(member).short().to_owned(),
                cast.name.to_owned(),
            ],
            cwd: None,
            env_remove: Vec::new(),
            // The stand-in says it itself.
            hint: None,
        })
    }

    /// The changes of an agent at `at`: a summary, or a small patch.
    fn diff_at(&self, agent: AgentId, full: bool, at: Timestamp) -> Result<String, SourceError> {
        let registry = Registry::from_events(&self.events_until(at))
            .map_err(|error| SourceError::Failed(error.to_string()))?;
        let record = registry
            .agent(agent)
            .ok_or(SourceError::UnknownAgent(agent))?;
        let stat = record.delivery.diff;
        if stat.files == 0 {
            return Ok(String::new());
        }
        let file = format!("src/{}.rs", record.name.replace('-', "_"));
        let summary = format!(
            " {file} | {} {}\n {} files changed, {} insertions(+), {} deletions(-)\n",
            stat.insertions + stat.deletions,
            "+".repeat(8),
            stat.files,
            stat.insertions,
            stat.deletions,
        );
        if !full {
            return Ok(summary);
        }
        Ok(format!(
            "diff --git a/{file} b/{file}\n--- a/{file}\n+++ b/{file}\n@@ -1,3 +1,4 @@\n \
             // {name}\n-let total = 0;\n+let total_cents: i64 = 0;\n+let currency = \"EUR\";\n",
            name = record.name
        ))
    }

    /// The events that post a message from the person to an agent (and
    /// open their thread the first time), with the channel to send them on.
    fn message_events(
        &self,
        member: usize,
        message: Outgoing,
        now: Timestamp,
    ) -> Result<(Updates, Vec<Event>), SourceError> {
        let Outgoing {
            to,
            kind,
            body,
            in_reply_to,
        } = message;
        let mut outbox = self.outbox();
        let sender = outbox
            .sender
            .clone()
            .ok_or_else(|| SourceError::Disconnected("not subscribed yet".to_owned()))?;
        outbox.sent += 1;
        let random = 0x0e00_0000 + outbox.sent;
        let at = u64::try_from(now.as_millis()).unwrap_or(0);
        let mut events = Vec::new();
        let known = outbox
            .threads
            .iter()
            .find(|(agent, _)| *agent == to)
            .map(|(_, thread)| *thread);
        let thread = if let Some(thread) = known {
            thread
        } else {
            let thread = ThreadId::from_ulid(Ulid::from_parts(at, random));
            outbox.threads.push((to, thread));
            events.push(Event::new(
                now,
                EventKind::ThreadOpened {
                    thread: Thread {
                        id: thread,
                        title: format!("You and {}", scenario::CAST[member].name),
                        mission: None,
                        participants: vec![Party::Human, Party::Agent(to)],
                        opened_at: now,
                    },
                },
            ));
            thread
        };
        events.push(Event::new(
            now,
            EventKind::MessagePosted {
                message: Message {
                    id: MessageId::from_ulid(Ulid::from_parts(at, random + 1)),
                    thread,
                    from: Party::Human,
                    kind,
                    body,
                    refs: Vec::new(),
                    in_reply_to,
                    at: now,
                    receipts: vec![Receipt {
                        to: Party::Agent(to),
                        delivered_at: None,
                        read_at: None,
                    }],
                },
            },
        ));
        Ok((sender, events))
    }
}

#[cfg(test)]
pub(crate) use scenario::{
    BROKEN_LINKS, EXPLORE_HANDLERS, EXPLORE_TRACES, INFRA, INVOICE_PAGE, INVOICE_SCHEMA,
    INVOICE_TOOLS, INVOICING, LATENCY_AUDIT, TOWER, TRACE_GATEWAY, TRACE_SEARCH, agent_id,
};

#[cfg(test)]
mod tests {
    use volee_core::message::MessageKind;

    use super::*;
    use crate::clock::ManualClock;

    fn to(agent: AgentId, kind: MessageKind, body: &str) -> Outgoing {
        Outgoing {
            to: agent,
            kind,
            body: body.to_owned(),
            in_reply_to: None,
        }
    }

    fn start() -> Timestamp {
        Timestamp::from_millis(1_793_986_617_000)
    }

    fn demo() -> (DemoSource, ManualClock) {
        let clock = ManualClock::new(start());
        (DemoSource::new(Arc::new(clock.clone())), clock)
    }

    #[tokio::test(start_paused = true)]
    async fn the_stream_delivers_beats_in_time() {
        let (demo, _clock) = demo();
        let Subscription {
            snapshot,
            mut updates,
        } = demo.subscribe().await.unwrap();
        assert_eq!(snapshot, demo.events_until(start()));
        let first = updates.recv().await.unwrap().unwrap();
        assert_eq!(first.at, start().plus_millis(2_000));
        let second = updates.recv().await.unwrap().unwrap();
        assert!(second.at > first.at);
        let mut registry = Registry::from_events(&snapshot).unwrap();
        registry.apply(&first).unwrap();
        registry.apply(&second).unwrap();
    }

    #[tokio::test]
    async fn previews_follow_the_state() {
        let (demo, clock) = demo();
        let schema = agent_id(start(), INVOICE_SCHEMA);
        let screen = demo.preview(schema).await.unwrap();
        assert!(screen.contains("bun run db:migrate"));
        assert!(screen.contains("Do you want to proceed?"));
        clock.advance(10_000);
        let screen = demo.preview(schema).await.unwrap();
        assert!(!screen.contains("Do you want to proceed?"));
        assert!(screen.contains("esc to interrupt"));
        let stranger = AgentId::from_ulid(Ulid::from_parts(1, 1));
        assert!(matches!(
            demo.preview(stranger).await,
            Err(SourceError::UnknownAgent(_))
        ));
    }

    #[tokio::test]
    async fn messages_from_the_person_come_back_as_events() {
        let (demo, _clock) = demo();
        let tools = agent_id(start(), INVOICE_TOOLS);
        assert!(matches!(
            demo.send_message(to(tools, MessageKind::Answer, "Yes"))
                .await,
            Err(SourceError::Disconnected(_))
        ));
        let Subscription {
            snapshot,
            mut updates,
        } = demo.subscribe().await.unwrap();
        let question = MessageId::from_ulid(Ulid::from_parts(1, 1));
        demo.send_message(Outgoing {
            in_reply_to: Some(question),
            ..to(tools, MessageKind::Answer, "Yes, false by default.")
        })
        .await
        .unwrap();
        demo.send_message(to(tools, MessageKind::Note, "Thanks"))
            .await
            .unwrap();
        let mut registry = Registry::from_events(&snapshot).unwrap();
        for _ in 0..3 {
            registry
                .apply(&updates.recv().await.unwrap().unwrap())
                .unwrap();
        }
        let thread = registry
            .thread_between(Party::Human, Party::Agent(tools))
            .unwrap();
        let messages = registry.thread_messages(thread.id);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].in_reply_to, Some(question));
    }

    #[tokio::test]
    async fn only_background_sessions_can_be_taken_over() {
        let (demo, _clock) = demo();
        let take_over = demo.attach(agent_id(start(), INVOICE_PAGE)).await.unwrap();
        assert_eq!(take_over.program, "sh");
        assert_eq!(
            take_over.args.last().map(String::as_str),
            Some("invoice-page")
        );
        assert_eq!(
            demo.attach(agent_id(start(), INFRA)).await,
            Err(SourceError::Unsupported(
                "it runs in another terminal; switch to it there".into()
            ))
        );
        assert_eq!(
            demo.attach(agent_id(start(), EXPLORE_TRACES)).await,
            Err(SourceError::Unsupported(
                "it runs inside its parent's session".into()
            ))
        );
    }

    #[tokio::test]
    async fn diffs_summarize_the_delivery() {
        let (demo, _clock) = demo();
        let page = agent_id(start(), INVOICE_PAGE);
        let stat = demo.diff(page, false).await.unwrap();
        assert!(
            stat.text
                .contains("9 files changed, 163 insertions(+), 43 deletions(-)")
        );
        assert!(!stat.truncated);
        let patch = demo.diff(page, true).await.unwrap();
        assert!(patch.text.starts_with("diff --git"));
        let tower = demo.diff(agent_id(start(), TOWER), false).await.unwrap();
        assert_eq!(tower, Changes::default());
    }
}
