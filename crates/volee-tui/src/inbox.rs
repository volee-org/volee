//! The person's messages (ADR 0006): every thread they take part in, the
//! most recent activity first, under the questions they have not answered
//! yet, which stay pinned on top until an answer refers to them.
//!
//! The window lists those rows on the left and shows the selected thread on
//! the right. `Enter` reads the thread (and marks its messages read), `r`
//! replies, `s` asks for a summary.

use volee_core::message::{MessageKind, Party};
use volee_core::time::Timestamp;
use volee_core::{AgentId, MessageId, Registry, ThreadId};

use crate::app::{Draft, Effect, Model, Overlay};
use crate::editor::TextField;
use crate::keymap::Action;
use crate::source::{SourceError, Summary};

/// One row of the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    /// A question to the person that no answer refers to yet.
    Question(MessageId),
    /// A thread the person takes part in.
    Thread(ThreadId),
}

/// The messages window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct InboxView {
    /// The selected row; the first one when it is gone.
    pub(crate) selected: Option<Row>,
    /// The keyboard is in the thread, to scroll it, rather than in the list.
    pub(crate) reading: bool,
    /// Lines scrolled up from the end of the thread.
    pub(crate) scroll: u16,
    /// The summary asked for, with the thread it is about.
    pub(crate) summary: Option<(ThreadId, SummaryState)>,
}

/// Where a summary stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SummaryState {
    /// Asked for.
    Loading,
    /// Written.
    Ready(Summary),
    /// Refused or failed, with the source's sentence.
    Failed(String),
}

/// What a reply goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Reply {
    /// The agent.
    pub(crate) to: AgentId,
    /// The thread it goes in.
    pub(crate) thread: ThreadId,
    /// The question it answers.
    pub(crate) question: Option<MessageId>,
}

/// Questions waiting for the person and messages they have not read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    /// Questions no answer refers to.
    pub(crate) questions: usize,
    /// Messages to the person not read yet.
    pub(crate) unread: usize,
}

/// The rows of the list: unanswered questions, oldest first, then the
/// person's threads, the latest activity first.
pub(crate) fn rows(registry: &Registry) -> Vec<Row> {
    let mut rows: Vec<Row> = registry
        .unanswered_questions()
        .into_iter()
        .map(|question| Row::Question(question.id))
        .collect();
    let mut threads: Vec<(Timestamp, ThreadId)> = registry
        .threads()
        .filter(|thread| {
            thread.participants.contains(&Party::Human)
                || registry
                    .thread_messages(thread.id)
                    .iter()
                    .any(|message| message.from == Party::Human || message.is_for(Party::Human))
        })
        .map(|thread| (last_activity(registry, thread.id), thread.id))
        .collect();
    threads.sort_by(|a, b| b.cmp(a));
    rows.extend(threads.into_iter().map(|(_, thread)| Row::Thread(thread)));
    rows
}

/// When a thread last changed: its latest message, or its opening.
pub(crate) fn last_activity(registry: &Registry, thread: ThreadId) -> Timestamp {
    registry
        .thread_messages(thread)
        .last()
        .map(|message| message.at)
        .or_else(|| registry.thread(thread).map(|thread| thread.opened_at))
        .unwrap_or(Timestamp::from_millis(0))
}

/// The thread a row shows.
pub(crate) fn thread_of(registry: &Registry, row: Row) -> Option<ThreadId> {
    match row {
        Row::Question(id) => registry.message(id).map(|message| message.thread),
        Row::Thread(thread) => Some(thread),
    }
}

/// Messages of a thread (all threads when `None`) the person has not read.
pub(crate) fn unread(registry: &Registry, thread: Option<ThreadId>) -> Vec<MessageId> {
    let messages = match thread {
        Some(thread) => registry.thread_messages(thread),
        None => registry
            .threads()
            .flat_map(|t| registry.thread_messages(t.id))
            .collect(),
    };
    messages
        .into_iter()
        .filter(|message| {
            message.from != Party::Human
                && message
                    .receipt(Party::Human)
                    .is_some_and(|receipt| receipt.read_at.is_none())
        })
        .map(|message| message.id)
        .collect()
}

/// The counts of the top bar.
pub(crate) fn counts(registry: &Registry) -> Counts {
    Counts {
        questions: registry.unanswered_questions().len(),
        unread: unread(registry, None).len(),
    }
}

/// Who a reply from a row goes to: the asker of a question, else the last
/// agent that wrote in the thread, its mission's coordinator, or the agent
/// taking part. volee's own alerts take no reply.
pub(crate) fn reply(registry: &Registry, row: Row) -> Option<Reply> {
    if let Row::Question(id) = row {
        let question = registry.message(id)?;
        let Party::Agent(to) = question.from else {
            return None;
        };
        return Some(Reply {
            to,
            thread: question.thread,
            question: Some(id),
        });
    }
    let thread = thread_of(registry, row)?;
    let record = registry.thread(thread)?;
    let to = registry
        .thread_messages(thread)
        .iter()
        .rev()
        .find_map(|message| match message.from {
            Party::Agent(agent) => Some(agent),
            Party::Human | Party::System => None,
        })
        .or_else(|| {
            record
                .mission
                .and_then(|mission| registry.mission(mission))
                .and_then(|mission| mission.coordinator)
        })
        .or_else(|| {
            record.participants.iter().find_map(|party| match party {
                Party::Agent(agent) => Some(*agent),
                Party::Human | Party::System => None,
            })
        })?;
    let question = registry
        .unanswered_questions()
        .into_iter()
        .rfind(|message| message.thread == thread && message.from == Party::Agent(to))
        .map(|message| message.id);
    Some(Reply {
        to,
        thread,
        question,
    })
}

impl InboxView {
    /// The selected row, or the first one when it is gone.
    pub(crate) fn current(&self, rows: &[Row]) -> Option<Row> {
        self.selected
            .filter(|row| rows.contains(row))
            .or_else(|| rows.first().copied())
    }
}

/// Opens the window on the first row: the oldest question, else the most
/// recent thread.
pub(crate) fn open(model: &mut Model) {
    let selected = rows(&model.registry).first().copied();
    model.overlay = Some(Overlay::Inbox(InboxView {
        selected,
        ..InboxView::default()
    }));
}

/// The window's keys.
pub(crate) fn perform(model: &mut Model, action: Action, effects: &mut Vec<Effect>) {
    let rows = rows(&model.registry);
    let Some(Overlay::Inbox(view)) = &mut model.overlay else {
        return;
    };
    let current = view.current(&rows);
    match action {
        Action::Down | Action::Up | Action::First | Action::Last if view.reading => {
            view.scroll = match action {
                Action::Up => view.scroll.saturating_add(1),
                Action::Down => view.scroll.saturating_sub(1),
                Action::First => u16::MAX,
                _ => 0,
            };
        }
        Action::Down | Action::Up | Action::First | Action::Last => {
            let index = current.and_then(|row| rows.iter().position(|r| *r == row));
            let next = match (action, index) {
                (Action::First, _) | (_, None) => rows.first(),
                (Action::Last, _) => rows.last(),
                (Action::Down, Some(index)) => rows.get(index + 1).or(rows.get(index)),
                (_, Some(index)) => rows.get(index.saturating_sub(1)),
            };
            if next.copied() != current {
                view.selected = next.copied();
                view.scroll = 0;
            }
        }
        Action::Apply => {
            view.reading = true;
            view.scroll = 0;
            if let Some(thread) = current.and_then(|row| thread_of(&model.registry, row)) {
                effects.extend(
                    unread(&model.registry, Some(thread))
                        .into_iter()
                        .map(Effect::MarkRead),
                );
            }
        }
        Action::Back => {
            if view.reading {
                view.reading = false;
            } else {
                model.overlay = None;
            }
        }
        Action::Summarize => {
            if let Some(thread) = current.and_then(|row| thread_of(&model.registry, row)) {
                view.summary = Some((thread, SummaryState::Loading));
                effects.push(Effect::Summarize(thread));
            }
        }
        Action::Reply => match current.and_then(|row| reply(&model.registry, row)) {
            Some(reply) => {
                let back = model.overlay.take().map(Box::new);
                write_reply(model, reply, back);
            }
            None => model.notify("volee's alerts take no reply.", false),
        },
        _ => {}
    }
}

/// Opens the composer on a reply, keeping a draft written to the same
/// agent; the messages window comes back when the composer closes.
fn write_reply(model: &mut Model, reply: Reply, back: Option<Box<Overlay>>) {
    let kind = if reply.question.is_some() {
        MessageKind::Answer
    } else {
        MessageKind::Note
    };
    let field = match model.draft.take() {
        Some(draft) if draft.to == reply.to && !draft.sending => draft.field,
        _ => TextField::default(),
    };
    model.draft = Some(Draft {
        to: reply.to,
        kind,
        field,
        question: reply.question,
        thread: Some(reply.thread),
        sending: false,
        back,
    });
    model.overlay = Some(Overlay::Composer);
}

/// A summary arrived, or the reason there is none.
pub(crate) fn summarized(
    model: &mut Model,
    thread: ThreadId,
    result: Result<Summary, SourceError>,
) -> bool {
    let Some(Overlay::Inbox(view)) = &mut model.overlay else {
        return false;
    };
    match &mut view.summary {
        Some((asked, state)) if *asked == thread => {
            *state = match result {
                Ok(summary) => SummaryState::Ready(summary),
                Err(error) => SummaryState::Failed(error.to_string()),
            };
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::clock::ManualClock;
    use crate::demo::{DemoSource, INVOICE_TOOLS, agent_id};

    fn start() -> Timestamp {
        Timestamp::from_millis(1_793_986_617_000)
    }

    fn registry() -> Registry {
        let demo = DemoSource::new(Arc::new(ManualClock::new(start())));
        Registry::from_events(&demo.events_until(start())).unwrap()
    }

    #[test]
    fn questions_come_first_then_threads_by_activity() {
        let registry = registry();
        let rows = rows(&registry);
        let Row::Question(question) = rows[0] else {
            panic!("no pinned question: {rows:?}");
        };
        let asked = registry.message(question).unwrap();
        assert_eq!(asked.from, Party::Agent(agent_id(start(), INVOICE_TOOLS)));
        let titles: Vec<&str> = rows[1..]
            .iter()
            .map(|row| {
                let thread = thread_of(&registry, *row).unwrap();
                registry.thread(thread).unwrap().title.as_str()
            })
            .collect();
        // volee's supervisor wrote in Invoicing 59 seconds ago, the
        // question reached the inbox a minute ago, volee's alert seven
        // minutes ago, the audit's contract before.
        assert_eq!(titles, ["Invoicing", "Inbox", "volee", "Latency audit"]);
        let times: Vec<Timestamp> = rows[1..]
            .iter()
            .map(|row| last_activity(&registry, thread_of(&registry, *row).unwrap()))
            .collect();
        assert!(times.windows(2).all(|pair| pair[0] >= pair[1]), "{times:?}");
    }

    #[test]
    fn counts_and_unread_messages_are_the_persons() {
        let registry = registry();
        assert_eq!(
            counts(&registry),
            Counts {
                questions: 1,
                unread: 2
            }
        );
        let Row::Question(question) = rows(&registry)[0] else {
            panic!("no question");
        };
        let thread = registry.message(question).unwrap().thread;
        assert_eq!(unread(&registry, Some(thread)), [question]);
    }

    /// The row of the thread with a title.
    fn titled(registry: &Registry, rows: &[Row], title: &str) -> Row {
        *rows
            .iter()
            .find(|row| {
                matches!(row, Row::Thread(thread)
                    if registry.thread(*thread).is_some_and(|t| t.title == title))
            })
            .unwrap_or_else(|| panic!("no thread {title}"))
    }

    #[test]
    fn replies_go_to_the_asker_or_the_last_agent_and_not_to_volee() {
        let registry = registry();
        let rows = rows(&registry);
        let tools = agent_id(start(), INVOICE_TOOLS);
        let answer = reply(&registry, rows[0]).unwrap();
        assert_eq!(answer.to, tools);
        assert!(answer.question.is_some());
        // The inbox thread: the same agent, still answering its question.
        let inbox = reply(&registry, titled(&registry, &rows, "Inbox")).unwrap();
        assert_eq!(inbox, answer);
        // A mission thread: the last agent that wrote there, not volee.
        let mission = reply(&registry, titled(&registry, &rows, "Invoicing")).unwrap();
        assert_eq!(mission.question, None);
        assert_eq!(registry.agent(mission.to).unwrap().name, "invoice-page");
        // volee's alerts take no reply.
        assert_eq!(reply(&registry, titled(&registry, &rows, "volee")), None);
    }

    mod keys {
        use ratatui::crossterm::event::KeyCode;

        use super::*;
        use crate::app::{Msg, update};
        use crate::keymap::Context;
        use crate::source::Outgoing;
        use crate::test_support::{id, key, model, press};

        fn view(model: &Model) -> &InboxView {
            match &model.overlay {
                Some(Overlay::Inbox(view)) => view,
                other => panic!("no messages window: {other:?}"),
            }
        }

        #[test]
        fn i_opens_the_messages_and_enter_reads_a_thread() {
            let mut model = model(120, 40);
            press(&mut model, "i");
            assert_eq!(model.contexts(), [Context::Inbox]);
            let rows = rows(&model.registry);
            assert_eq!(view(&model).selected, Some(rows[0]), "the question first");
            let Row::Question(question) = rows[0] else {
                panic!("no question first");
            };
            assert_eq!(
                key(&mut model, KeyCode::Enter),
                [Effect::MarkRead(question)]
            );
            assert!(view(&model).reading);
            press(&mut model, "k");
            assert_eq!(view(&model).scroll, 1, "j and k scroll the thread");
            key(&mut model, KeyCode::Esc);
            assert!(!view(&model).reading);
            press(&mut model, "jj");
            assert_eq!(view(&model).selected, Some(rows[2]));
            key(&mut model, KeyCode::Esc);
            assert_eq!(model.overlay, None);
        }

        #[test]
        fn r_answers_the_question_and_comes_back_to_the_messages() {
            let mut model = model(120, 40);
            let tools = id(INVOICE_TOOLS);
            press(&mut model, "ir");
            assert_eq!(model.overlay, Some(Overlay::Composer));
            let draft = model.draft.clone().unwrap();
            assert_eq!(draft.to, tools);
            assert_eq!(draft.kind, MessageKind::Answer);
            press(&mut model, "Yes");
            let effects = key(&mut model, KeyCode::Enter);
            assert_eq!(
                effects,
                [Effect::Send(Outgoing {
                    to: tools,
                    kind: MessageKind::Answer,
                    body: "Yes".into(),
                    in_reply_to: draft.question,
                    thread: draft.thread,
                })]
            );
            update(
                &mut model,
                Msg::MessageSent {
                    to: tools,
                    result: Ok(()),
                },
            );
            assert!(
                matches!(model.overlay, Some(Overlay::Inbox(_))),
                "back to the messages"
            );
            // Closing a composer opened from the messages goes back there too.
            press(&mut model, "r");
            key(&mut model, KeyCode::Esc);
            assert!(matches!(model.overlay, Some(Overlay::Inbox(_))));
        }

        #[test]
        fn s_summarizes_the_thread_or_says_why_not() {
            let mut model = model(120, 40);
            press(&mut model, "i");
            let thread = thread_of(&model.registry, rows(&model.registry)[0]).unwrap();
            assert_eq!(press(&mut model, "s"), [Effect::Summarize(thread)]);
            assert_eq!(view(&model).summary, Some((thread, SummaryState::Loading)));
            let refusal = "thread summaries are off: set `summaries = true` under [messages] \
                           in the configuration";
            update(
                &mut model,
                Msg::Summarized {
                    thread,
                    result: Err(SourceError::Unsupported(refusal.into())),
                },
            );
            assert_eq!(
                view(&model).summary,
                Some((thread, SummaryState::Failed(refusal.into())))
            );
        }

        #[test]
        fn volee_s_alerts_take_no_reply() {
            let mut model = model(120, 40);
            press(&mut model, "ijjjr");
            let rows = rows(&model.registry);
            assert_eq!(view(&model).selected, Some(rows[3]));
            assert_eq!(
                model.notice.as_ref().unwrap().text,
                "volee's alerts take no reply."
            );
        }
    }
}
