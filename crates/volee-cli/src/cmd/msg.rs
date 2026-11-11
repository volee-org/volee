//! `volee msg send`, `volee msg questions` and `volee msg answer`.

use std::fmt::Write as _;

use anyhow::{Context, Result, bail};
use volee_core::agent::Hold;
use volee_core::message::{Message, MessageKind, Party};
use volee_core::time::{Elapsed, Timestamp};
use volee_core::{Agent, AgentState, Event, Registry};
use volee_daemon::{Clock, SystemClock};
use volee_proto::methods::{EventsSince, SendMessage, SendParams, SinceParams};

use crate::connect::connect_or_start;
use crate::{resolve, text};

/// Sends a message from the human to an agent, named as a person types it.
pub(crate) async fn send(to: &str, body: String, kind: MessageKind) -> Result<()> {
    resolve::as_the_person("writes in their own name")?;
    let client = connect_or_start().await?;
    let batch = client.call::<EventsSince>(SinceParams::default()).await?;
    let events: Vec<Event> = batch.events.into_iter().map(|e| e.event).collect();
    let registry = Registry::from_events(&events)
        .context("the daemon sent events that do not fit together")?;
    let to = resolve::agent(&registry, to)?;
    let (name, outlook) = registry.agent(to).map_or_else(
        || (to.short(), String::new()),
        |agent| {
            (
                text::one_line(&agent.name).into_owned(),
                format!(" {}", outlook(agent)),
            )
        },
    );
    let sent = client
        .call::<SendMessage>(SendParams {
            from: Party::Human,
            to: vec![Party::Agent(to)],
            kind,
            body,
            refs: vec![],
            in_reply_to: None,
            options: Vec::new(),
            thread: None,
        })
        .await?;
    println!("Sent {} to {name}.{outlook}", sent.message.short());
    Ok(())
}

/// Prints the questions agents asked the human that have no answer yet,
/// oldest first, with the answers each one offers.
pub(crate) async fn questions() -> Result<()> {
    let client = connect_or_start().await?;
    let registry = resolve::registry(&client).await?;
    print!("{}", waiting(&registry, SystemClock.now()));
    Ok(())
}

/// The questions waiting for the human, as `volee msg questions` prints
/// them: who asks, since when, the question, the answers it offers with
/// their numbers, and the command that answers it.
fn waiting(registry: &Registry, now: Timestamp) -> String {
    let questions = registry.unanswered_questions();
    if questions.is_empty() {
        return "No question waits for you.\n".to_owned();
    }
    let mut out = String::new();
    for question in questions {
        let asker = match question.from {
            Party::Agent(agent) => registry.agent(agent).map_or_else(
                || agent.short(),
                |agent| text::one_line(&agent.name).into_owned(),
            ),
            other => other.to_string(),
        };
        let id = question.id.short();
        // Writing to a String cannot fail.
        let _ = writeln!(
            out,
            "{id}  {asker} asks, {} ago:",
            Elapsed::between(question.at, now)
        );
        for line in text::printable(&question.body).lines() {
            let _ = writeln!(out, "  {line}");
        }
        if question.options.is_empty() {
            let _ = writeln!(
                out,
                "  Answer with `volee msg answer {id} \"<your answer>\"`."
            );
        } else {
            let offered: Vec<String> = (1..)
                .zip(&question.options)
                .map(|(number, option)| format!("{number} {}", text::one_line(option)))
                .collect();
            let _ = writeln!(out, "  Answers: {}", offered.join(" \u{b7} "));
            let _ = writeln!(out, "  Answer with `volee msg answer {id} <number>`.");
        }
    }
    out
}

/// Answers a question an agent asked the human: with one of the answers it
/// offers, named by its number or its words, or in the human's own words.
pub(crate) async fn answer(question: &str, typed: &str) -> Result<()> {
    resolve::as_the_person("answers a question asked to them")?;
    let client = connect_or_start().await?;
    let registry = resolve::registry(&client).await?;
    let question = resolve::question(&registry, question)?;
    let asked = registry
        .message(question)
        .context("the question is gone from the daemon's log")?;
    let Party::Agent(to) = asked.from else {
        bail!("volee's own messages take no answer");
    };
    let body = answer_text(asked, typed)?;
    let offered = asked.chosen(&body).is_some();
    let (name, outlook) = registry.agent(to).map_or_else(
        || (to.short(), String::new()),
        |agent| {
            (
                text::one_line(&agent.name).into_owned(),
                format!(" {}", outlook(agent)),
            )
        },
    );
    client
        .call::<SendMessage>(SendParams {
            from: Party::Human,
            to: vec![Party::Agent(to)],
            kind: MessageKind::Answer,
            body: body.clone(),
            refs: vec![],
            in_reply_to: Some(question),
            options: Vec::new(),
            thread: Some(asked.thread),
        })
        .await?;
    println!("Answered \"{}\" to {name}.{outlook}", text::one_line(&body));
    if !asked.options.is_empty() && !offered {
        println!(
            "That is not one of the answers it offers ({}): {name} reads it as your own words.",
            asked.options.join(", ")
        );
    }
    Ok(())
}

/// What an answer typed on the command line says: the answer the question
/// offers under that number, or with those words, in the question's own
/// spelling; else the words as typed.
fn answer_text(question: &Message, typed: &str) -> Result<String> {
    let typed = typed.trim();
    if typed.is_empty() {
        bail!("an answer needs words");
    }
    if let Some(option) = question.chosen(typed) {
        return Ok(option.to_owned());
    }
    if !question.options.is_empty()
        && let Ok(number) = typed.parse::<usize>()
    {
        return match number.checked_sub(1).and_then(|n| question.options.get(n)) {
            Some(option) => Ok(option.clone()),
            None => bail!(
                "this question offers {} answers: {}",
                question.options.len(),
                (1..)
                    .zip(&question.options)
                    .map(|(number, option)| format!("{number} {}", text::one_line(option)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
    }
    Ok(typed.to_owned())
}

/// What happens to a message from the human to `agent`, in one sentence.
fn outlook(agent: &Agent) -> String {
    let name = text::one_line(&agent.name);
    if agent.joined() {
        // No hooks: only the agent's own read_messages call hands it over.
        return if agent.state.is_live() {
            format!(
                "{name} joined through volee's MCP server, which cannot hand it messages: it sees \
                 this one when it calls read_messages."
            )
        } else {
            format!(
                "{name}'s MCP server is not running: the message waits until {name} joins again \
                 and calls read_messages."
            )
        };
    }
    if !agent.managed() {
        return format!(
            "volee did not launch {name}, so nothing hands it this message: {name} sees it only if \
             it reads its messages with volee's MCP tools. To talk to it now, open it and type \
             there: `volee attach {name}`, or enter in the interface."
        );
    }
    if agent.generic() {
        return if agent.state.is_live() {
            format!(
                "{name} is a program started with `volee run`, which nothing hands a message to: \
                 to talk to it, open it and type there: `volee attach {name}`, or enter in the \
                 interface."
            )
        } else {
            format!(
                "{name} is a program started with `volee run`, and it ended: nothing resumes it, \
                 so nothing will read this message. Run it again to talk to it."
            )
        };
    }
    if agent.state.is_live() {
        return format!("It reaches {name} at the end of its turn, or wakes it if it is idle.");
    }
    let resumable = matches!(agent.state, AgentState::Stopped | AgentState::Lost)
        && agent.current_session().is_some();
    match agent.hold {
        Some(Hold::MissionOver { .. }) => format!(
            "{name}'s mission is over, so volee does not resume it: the message waits until {name} \
             runs again."
        ),
        // A message from the human lifts the hold of an agent it stopped.
        Some(Hold::StoppedByHuman { .. }) | None if resumable => {
            format!("{name} is not running: volee resumes it with this message.")
        }
        Some(Hold::StoppedByHuman { .. }) | None => format!(
            "{name} is not running ({}): the message waits until it runs again.",
            agent.state.label()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ulid::Ulid;
    use volee_core::harness::{HarnessKind, SessionKey, SessionOrigin};
    use volee_core::role::LaunchedBy;
    use volee_core::role::Role;
    use volee_core::time::Timestamp;
    use volee_core::workspace::{Location, Workspace};
    use volee_core::{AgentId, EventKind};

    fn agent(launched_by: LaunchedBy, extra: Vec<EventKind>) -> Agent {
        let id = AgentId::from_ulid(Ulid::from_parts(1, 1));
        let mut events = vec![EventKind::AgentRegistered {
            agent: id,
            name: "billing".into(),
            role: Role::Worker,
            harness: HarnessKind::ClaudeCode,
            launched_by,
            parent: None,
            mission: None,
            workspace: Workspace::default(),
            location: Location::Background,
        }];
        events.extend(extra);
        let events: Vec<Event> = events
            .into_iter()
            .map(|kind| Event::new(Timestamp::from_millis(1), kind))
            .collect();
        Registry::from_events(&events)
            .unwrap()
            .agent(id)
            .unwrap()
            .clone()
    }

    fn ended(id: AgentId) -> Vec<EventKind> {
        vec![
            EventKind::SessionLinked {
                agent: id,
                session: SessionKey::claude("first"),
                origin: SessionOrigin::Started,
            },
            EventKind::StateChanged {
                agent: id,
                state: AgentState::Stopped,
                detail: None,
            },
        ]
    }

    /// A registry where `billing` asked the human two questions: one that
    /// offers answers (message 1), one that does not (message 2).
    fn asked() -> (Registry, volee_core::MessageId, volee_core::MessageId) {
        use volee_core::message::{Receipt, Thread};
        use volee_core::{MessageId, ThreadId};

        let id = AgentId::from_ulid(Ulid::from_parts(1, 1));
        let thread = ThreadId::from_ulid(Ulid::from_parts(2, 1));
        let question = |n: u128, body: &str, options: &[&str]| EventKind::MessagePosted {
            message: Message {
                id: MessageId::from_ulid(Ulid::from_parts(3, n)),
                thread,
                from: Party::Agent(id),
                kind: MessageKind::Question,
                body: body.to_owned(),
                refs: vec![],
                in_reply_to: None,
                options: options.iter().map(|option| (*option).to_owned()).collect(),
                at: Timestamp::from_millis(1),
                receipts: vec![Receipt {
                    to: Party::Human,
                    delivered_at: None,
                    read_at: None,
                }],
            },
        };
        let events = [
            EventKind::AgentRegistered {
                agent: id,
                name: "billing".into(),
                role: Role::Worker,
                harness: HarnessKind::ClaudeCode,
                launched_by: LaunchedBy::Human,
                parent: None,
                mission: None,
                workspace: Workspace::default(),
                location: Location::Background,
            },
            EventKind::ThreadOpened {
                thread: Thread {
                    id: thread,
                    title: "the human, billing (worker)".into(),
                    mission: None,
                    participants: vec![Party::Human, Party::Agent(id)],
                    opened_at: Timestamp::from_millis(1),
                },
            },
            question(
                1,
                "Both pull requests are green.\nMerge them?",
                &["ok merge", "no"],
            ),
            question(2, "HT or TTC?", &[]),
        ]
        .map(|kind| Event::new(Timestamp::from_millis(1), kind));
        (
            Registry::from_events(&events).unwrap(),
            MessageId::from_ulid(Ulid::from_parts(3, 1)),
            MessageId::from_ulid(Ulid::from_parts(3, 2)),
        )
    }

    #[test]
    fn the_questions_are_listed_with_the_answers_they_offer() {
        let (registry, offering, free) = asked();
        let text = waiting(&registry, Timestamp::from_millis(121_000));
        let expected = format!(
            "{offering}  billing asks, 2 min ago:\n  Both pull requests are green.\n  Merge \
             them?\n  Answers: 1 ok merge \u{b7} 2 no\n  Answer with `volee msg answer {offering} \
             <number>`.\n{free}  billing asks, 2 min ago:\n  HT or TTC?\n  Answer with `volee msg \
             answer {free} \"<your answer>\"`.\n",
            offering = offering.short(),
            free = free.short(),
        );
        assert_eq!(text, expected);
        assert_eq!(
            waiting(&Registry::new(), Timestamp::from_millis(1)),
            "No question waits for you.\n"
        );
    }

    #[test]
    fn an_answer_is_a_number_the_words_of_an_option_or_the_person_s_own() {
        let (registry, offering, free) = asked();
        let offering = registry.message(offering).unwrap();
        let free = registry.message(free).unwrap();
        assert_eq!(answer_text(offering, "1").unwrap(), "ok merge");
        assert_eq!(answer_text(offering, " 2 ").unwrap(), "no");
        // The option's own spelling, whatever the person typed.
        assert_eq!(answer_text(offering, "OK MERGE").unwrap(), "ok merge");
        // Their own words, as typed.
        assert_eq!(
            answer_text(offering, "merge the first one only").unwrap(),
            "merge the first one only"
        );
        let beyond = answer_text(offering, "3").unwrap_err().to_string();
        assert_eq!(beyond, "this question offers 2 answers: 1 ok merge, 2 no");
        assert!(answer_text(offering, "0").is_err());
        assert!(answer_text(offering, "  ").is_err());
        // Without answers offered, a number is an answer like any other.
        assert_eq!(answer_text(free, "20").unwrap(), "20");
    }

    #[test]
    fn a_question_is_named_by_its_id_or_by_the_agent_that_asked_it() {
        let (registry, offering, free) = asked();
        assert_eq!(
            resolve::question(&registry, &offering.to_string()).unwrap(),
            offering
        );
        assert_eq!(resolve::question(&registry, &free.short()).unwrap(), free);
        // The agent asked two: its name does not say which.
        let both = resolve::question(&registry, "billing")
            .unwrap_err()
            .to_string();
        assert!(both.contains("could be"), "{both}");
        assert!(both.contains(&offering.short()), "{both}");
        let nobody = resolve::question(&registry, "nobody")
            .unwrap_err()
            .to_string();
        assert!(nobody.contains("volee msg questions"), "{nobody}");
    }

    #[test]
    fn the_outlook_never_prints_control_characters() {
        let mut named = agent(LaunchedBy::Outside, vec![]);
        // A name a coordinator chose, with a window title change in it.
        named.name = "bill\u{1b}]0;owned\u{7}ing".into();
        let line = outlook(&named);
        assert!(!line.chars().any(char::is_control), "{line:?}");
        assert!(line.contains("bill]0;owneding"), "{line}");
    }

    #[test]
    fn the_outlook_says_whether_the_message_will_arrive() {
        let id = AgentId::from_ulid(Ulid::from_parts(1, 1));
        let working = agent(
            LaunchedBy::Human,
            vec![EventKind::StateChanged {
                agent: id,
                state: AgentState::Working,
                detail: None,
            }],
        );
        assert!(outlook(&working).contains("end of its turn"));

        let outside = agent(LaunchedBy::Outside, vec![]);
        assert!(outlook(&outside).contains("did not launch billing"));
        // Adopted into a host, volee hands it messages and resumes it.
        let host = |id| EventKind::LocationChanged {
            agent: id,
            location: Location::Host { id: "h1".into() },
        };
        let adopted = agent(LaunchedBy::Outside, vec![host(id)]);
        assert!(outlook(&adopted).contains("wakes it if it is idle"));
        let mut adopted_stopped = vec![host(id)];
        adopted_stopped.extend(ended(id));
        assert!(outlook(&agent(LaunchedBy::Outside, adopted_stopped)).contains("volee resumes it"));

        let stopped = agent(LaunchedBy::Human, ended(id));
        assert!(outlook(&stopped).contains("volee resumes it"));

        let mut held = ended(id);
        held.push(EventKind::StopRequested { agent: id });
        assert!(
            outlook(&agent(LaunchedBy::Human, held)).contains("volee resumes it"),
            "the human's message lifts the hold"
        );

        let failed = agent(
            LaunchedBy::Human,
            vec![EventKind::StateChanged {
                agent: id,
                state: AgentState::Failed,
                detail: None,
            }],
        );
        assert!(outlook(&failed).contains("waits until it runs again"));
    }

    #[test]
    fn a_program_started_with_volee_run_is_typed_into() {
        let id = AgentId::from_ulid(Ulid::from_parts(1, 1));
        let run = |state| {
            let events = [
                EventKind::AgentRegistered {
                    agent: id,
                    name: "billing".into(),
                    role: Role::Independent,
                    harness: HarnessKind::Other("opencode".into()),
                    launched_by: LaunchedBy::Human,
                    parent: None,
                    mission: None,
                    workspace: Workspace::default(),
                    location: Location::Host { id: "h1".into() },
                },
                EventKind::StateChanged {
                    agent: id,
                    state,
                    detail: None,
                },
            ]
            .map(|kind| Event::new(Timestamp::from_millis(1), kind));
            Registry::from_events(&events)
                .unwrap()
                .agent(id)
                .unwrap()
                .clone()
        };
        // Nothing hands it the message: no promise that it arrives.
        for state in [AgentState::Working, AgentState::Idle] {
            let line = outlook(&run(state));
            assert!(line.contains("started with `volee run`"), "{line}");
            assert!(line.contains("`volee attach billing`"), "{line}");
            assert!(!line.contains("end of its turn"), "{line}");
        }
        // And once it ended nothing resumes it: no promise of that either,
        // and no advice to open what cannot be opened.
        let ended = outlook(&run(AgentState::Stopped));
        assert!(ended.contains("nothing resumes it"), "{ended}");
        assert!(ended.contains("Run it again"), "{ended}");
        assert!(!ended.contains("volee attach"), "{ended}");
    }

    #[test]
    fn a_joined_agent_reads_its_messages_itself() {
        let id = AgentId::from_ulid(Ulid::from_parts(1, 1));
        let joined = |extra: Vec<EventKind>| {
            let mut events = vec![EventKind::AgentRegistered {
                agent: id,
                name: "infra".into(),
                role: Role::Independent,
                harness: HarnessKind::Other("codex".into()),
                launched_by: LaunchedBy::Outside,
                parent: None,
                mission: None,
                workspace: Workspace::default(),
                location: Location::Unknown,
            }];
            events.extend(extra);
            let events: Vec<Event> = events
                .into_iter()
                .map(|kind| Event::new(Timestamp::from_millis(1), kind))
                .collect();
            Registry::from_events(&events)
                .unwrap()
                .agent(id)
                .unwrap()
                .clone()
        };
        let state = |state| EventKind::StateChanged {
            agent: id,
            state,
            detail: None,
        };
        let connected = outlook(&joined(vec![state(AgentState::Idle)]));
        assert!(
            connected.contains("when it calls read_messages"),
            "{connected}"
        );
        let gone = outlook(&joined(vec![state(AgentState::Stopped)]));
        assert!(gone.contains("joins again"), "{gone}");
    }
}
