//! `volee msg send`.

use anyhow::{Context, Result};
use volee_core::agent::Hold;
use volee_core::message::{MessageKind, Party};
use volee_core::{Agent, AgentState, Event, Registry};
use volee_proto::methods::{EventsSince, SendMessage, SendParams, SinceParams};

use crate::connect::connect_or_start;
use crate::{resolve, text};

/// Sends a message from the human to an agent, named as a person types it.
pub(crate) async fn send(to: &str, body: String, kind: MessageKind) -> Result<()> {
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
            thread: None,
        })
        .await?;
    println!("Sent {} to {name}.{outlook}", sent.message.short());
    Ok(())
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
