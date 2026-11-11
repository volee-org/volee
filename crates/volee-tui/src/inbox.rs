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

use crate::app::{Draft, Effect, Model, Overlay, Prompt};
use crate::editor::TextField;
use crate::keymap::Action;
use crate::source::{Outgoing, SourceError, Summary};

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

/// The question a row waits on, when it offers answers to choose from and
/// the person has not answered it: the row's own question, or the one a
/// reply from its thread would answer.
pub(crate) fn offered(registry: &Registry, row: Row) -> Option<&volee_core::message::Message> {
    let question = match row {
        Row::Question(id) => Some(id),
        Row::Thread(_) => reply(registry, row)?.question,
    }?;
    registry
        .message(question)
        .filter(|message| !message.options.is_empty() && registry.answer_to(question).is_none())
}

/// A digit typed in the messages window: asks, on the bottom line, whether
/// to send the answer of that number to the selected question. Returns
/// whether the digit was one of its answers.
pub(crate) fn pick(model: &mut Model, digit: u32) -> bool {
    let rows = rows(&model.registry);
    let Some(Overlay::Inbox(view)) = &model.overlay else {
        return false;
    };
    let Some(question) = view
        .current(&rows)
        .and_then(|row| offered(&model.registry, row))
    else {
        return false;
    };
    let Some(option) = usize::try_from(digit)
        .ok()
        .and_then(|digit| digit.checked_sub(1))
        .filter(|option| *option < question.options.len())
    else {
        return false;
    };
    model.prompt = Some(Prompt::Answer {
        question: question.id,
        option,
    });
    true
}

/// Sends the answer picked for a question, in the question's thread. The
/// answer is the option's own words, so that whoever asked can tell it from
/// a sentence that merely contains them.
pub(crate) fn answer(
    model: &mut Model,
    question: MessageId,
    option: usize,
    effects: &mut Vec<Effect>,
) {
    let Some(message) = model.registry.message(question) else {
        return;
    };
    let (Party::Agent(to), Some(body)) = (message.from, message.options.get(option)) else {
        return;
    };
    effects.push(Effect::Send(Outgoing {
        to,
        kind: MessageKind::Answer,
        body: body.clone(),
        in_reply_to: Some(question),
        thread: Some(message.thread),
    }));
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
        // Two questions wait, oldest first: the draft flag, then the
        // token calls.
        let askers: Vec<Party> = rows[..2]
            .iter()
            .map(|row| match row {
                Row::Question(id) => registry.message(*id).unwrap().from,
                Row::Thread(_) => panic!("a thread before the questions: {rows:?}"),
            })
            .collect();
        assert_eq!(
            askers,
            [
                Party::Agent(agent_id(start(), INVOICE_TOOLS)),
                Party::Agent(agent_id(start(), crate::demo::TRACE_GATEWAY)),
            ]
        );
        let titles: Vec<&str> = rows[2..]
            .iter()
            .map(|row| {
                let thread = thread_of(&registry, *row).unwrap();
                registry.thread(thread).unwrap().title.as_str()
            })
            .collect();
        // The token question reached the inbox 30 seconds ago, volee's
        // supervisor wrote in Invoicing a minute ago, volee's alert seven
        // minutes ago, the audit's contract before.
        assert_eq!(titles, ["Inbox", "Invoicing", "volee", "Latency audit"]);
        let times: Vec<Timestamp> = rows[2..]
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
                questions: 2,
                unread: 5
            }
        );
        let Row::Question(question) = rows(&registry)[0] else {
            panic!("no question");
        };
        // The inbox: four questions to the person, two of them answered
        // but never marked read.
        let thread = registry.message(question).unwrap().thread;
        let inbox = unread(&registry, Some(thread));
        assert_eq!(inbox.len(), 4);
        assert!(inbox.contains(&question));
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
        // The inbox thread: the last agent that asked there, answering its
        // question.
        let inbox = reply(&registry, titled(&registry, &rows, "Inbox")).unwrap();
        assert_eq!(inbox.to, agent_id(start(), crate::demo::TRACE_GATEWAY));
        assert_eq!(inbox.thread, answer.thread);
        assert!(inbox.question.is_some() && inbox.question != answer.question);
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
        use crate::app::Prompt;
        use crate::app::{Msg, update};
        use crate::keymap::Context;
        use crate::source::Outgoing;
        use crate::test_support::{id, key, model, offer, press};

        fn view(model: &Model) -> &InboxView {
            match &model.overlay {
                Some(Overlay::Inbox(view)) => view,
                other => panic!("no messages window: {other:?}"),
            }
        }

        #[test]
        fn capital_i_opens_the_messages_and_enter_reads_a_thread() {
            let mut model = model(120, 40);
            press(&mut model, "I");
            assert_eq!(model.contexts(), [Context::Inbox]);
            let rows = rows(&model.registry);
            assert_eq!(view(&model).selected, Some(rows[0]), "the question first");
            let Row::Question(question) = rows[0] else {
                panic!("no question first");
            };
            // Reading the inbox marks its four unread messages read.
            let effects = key(&mut model, KeyCode::Enter);
            assert_eq!(effects.len(), 4);
            assert!(effects.contains(&Effect::MarkRead(question)));
            assert!(
                effects
                    .iter()
                    .all(|effect| matches!(effect, Effect::MarkRead(_)))
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
            press(&mut model, "Ir");
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
        fn a_digit_picks_an_answer_the_question_offers_and_y_sends_it() {
            let mut model = model(120, 40);
            let tools = id(INVOICE_TOOLS);
            let question = offer(&mut model, &["ok merge", "no"]);
            let thread = model.registry.message(question).unwrap().thread;
            press(&mut model, "Ijj");
            assert_eq!(view(&model).selected, Some(Row::Question(question)));
            // A digit beyond its answers is not one of them.
            assert_eq!(press(&mut model, "3"), []);
            assert_eq!(press(&mut model, "0"), []);
            assert_eq!(model.prompt, None);
            // 1 asks before anything is sent.
            assert_eq!(press(&mut model, "1"), []);
            assert_eq!(
                model.prompt,
                Some(Prompt::Answer {
                    question,
                    option: 0
                })
            );
            assert_eq!(model.contexts(), [Context::Confirm]);
            // No: nothing was sent, and the messages are still there.
            assert_eq!(press(&mut model, "n"), []);
            assert_eq!(model.prompt, None);
            assert!(matches!(model.overlay, Some(Overlay::Inbox(_))));
            // 2, then yes: the answer is the option's own words, in reply
            // to the question, in its thread.
            press(&mut model, "2");
            assert_eq!(
                press(&mut model, "y"),
                [Effect::Send(Outgoing {
                    to: tools,
                    kind: MessageKind::Answer,
                    body: "no".into(),
                    in_reply_to: Some(question),
                    thread: Some(thread),
                })]
            );
            assert_eq!(model.prompt, None);
            assert!(matches!(model.overlay, Some(Overlay::Inbox(_))));
            // The person can still answer in their own words.
            press(&mut model, "r");
            assert_eq!(model.overlay, Some(Overlay::Composer));
            assert_eq!(model.draft.as_ref().unwrap().question, Some(question));
        }

        #[test]
        fn a_digit_does_nothing_on_a_question_without_answers_or_once_answered() {
            let mut model = model(120, 40);
            // The demo's questions offer nothing.
            press(&mut model, "I");
            assert_eq!(press(&mut model, "1"), []);
            assert_eq!(model.prompt, None);
            key(&mut model, KeyCode::Esc);

            let question = offer(&mut model, &["ok merge", "no"]);
            let asked = model.registry.message(question).unwrap().clone();
            assert_eq!(
                offered(&model.registry, Row::Question(question)).map(|message| message.id),
                Some(question)
            );
            // The thread's row answers it too: its latest question.
            let rows = rows(&model.registry);
            let row = *rows
                .iter()
                .find(|row| matches!(row, Row::Thread(thread) if *thread == asked.thread))
                .unwrap();
            assert_eq!(
                offered(&model.registry, row).map(|message| message.id),
                Some(question)
            );
            // Once the person answered, the digits pick nothing.
            let answer = volee_core::message::Message {
                id: MessageId::from_ulid(ulid::Ulid::from_parts(u64::MAX >> 16, 2)),
                from: Party::Human,
                kind: MessageKind::Answer,
                body: "ok merge".into(),
                in_reply_to: Some(question),
                options: Vec::new(),
                ..asked
            };
            let now = model.now;
            update(
                &mut model,
                Msg::Events(vec![volee_core::Event::new(
                    now,
                    volee_core::event::EventKind::MessagePosted { message: answer },
                )]),
            );
            assert_eq!(offered(&model.registry, row), None);
            press(&mut model, "I");
            assert_eq!(press(&mut model, "1"), []);
            assert_eq!(model.prompt, None);
        }

        #[test]
        fn s_summarizes_the_thread_or_says_why_not() {
            let mut model = model(120, 40);
            press(&mut model, "I");
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
            let rows = rows(&model.registry);
            let alerts = rows
                .iter()
                .position(|row| {
                    matches!(row, Row::Thread(thread)
                        if model.registry.thread(*thread).is_some_and(|t| t.title == "volee"))
                })
                .unwrap();
            press(&mut model, "I");
            press(&mut model, &"j".repeat(alerts));
            press(&mut model, "r");
            assert_eq!(view(&model).selected, Some(rows[alerts]));
            assert_eq!(
                model.notice.as_ref().unwrap().text,
                "volee's alerts take no reply."
            );
        }
    }
}
