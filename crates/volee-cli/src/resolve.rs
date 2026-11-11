//! Finding the agent or mission a person means: a full identifier, the
//! short form tables print (`ag_x7k2pq`), or a name.

use std::path::Path;

use anyhow::{Context, Result, bail};
use volee_config::Config;
use volee_core::message::{Message, Party};
use volee_core::{AgentId, Event, MessageId, MissionId, Registry};
use volee_proto::client::Client;
use volee_proto::methods::{EventsSince, SinceParams};

use crate::connect::paths;
use crate::text;

/// The shortest identifier ending accepted, so that a typo rarely matches.
const MIN_SUFFIX: usize = 4;

/// The daemon's registry, rebuilt from its event log.
pub(crate) async fn registry(client: &Client) -> Result<Registry> {
    let batch = client.call::<EventsSince>(SinceParams::default()).await?;
    let events: Vec<Event> = batch.events.into_iter().map(|e| e.event).collect();
    Registry::from_events(&events).context("the daemon sent events that do not fit together")
}

/// The agent `input` names: its identifier, its short identifier, or its
/// exact name.
pub(crate) fn agent(registry: &Registry, input: &str) -> Result<AgentId> {
    let input = input.trim();
    if let Ok(id) = input.parse::<AgentId>() {
        if registry.agent(id).is_some() {
            return Ok(id);
        }
        bail!("volee knows no agent {id}; see `volee ls`");
    }
    let named: Vec<(AgentId, &str)> = registry
        .agents()
        .filter(|agent| agent.name == input)
        .map(|agent| (agent.id, agent.name.as_str()))
        .collect();
    let candidates = if named.is_empty() {
        registry
            .agents()
            .filter(|agent| ends_with(&agent.id.to_string(), AgentId::PREFIX, input))
            .map(|agent| (agent.id, agent.name.as_str()))
            .collect()
    } else {
        named
    };
    match candidates.as_slice() {
        [(id, _)] => Ok(*id),
        [] => bail!("no agent is named `{input}` or has that id; see `volee ls`"),
        several => bail!(
            "`{input}` could be {}; use the id",
            several
                .iter()
                .map(|(id, name)| format!("{} ({})", text::one_line(name), id.short()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The mission `input` names: its identifier, its short identifier, or its
/// title (ignoring case).
pub(crate) fn mission(registry: &Registry, input: &str) -> Result<MissionId> {
    let input = input.trim();
    if let Ok(id) = input.parse::<MissionId>() {
        if registry.mission(id).is_some() {
            return Ok(id);
        }
        bail!("volee knows no mission {id}; see `volee mission ls --all`");
    }
    let titled: Vec<(MissionId, &str)> = registry
        .missions()
        .filter(|mission| mission.title.eq_ignore_ascii_case(input))
        .map(|mission| (mission.id, mission.title.as_str()))
        .collect();
    let candidates = if titled.is_empty() {
        registry
            .missions()
            .filter(|mission| ends_with(&mission.id.to_string(), MissionId::PREFIX, input))
            .map(|mission| (mission.id, mission.title.as_str()))
            .collect()
    } else {
        titled
    };
    match candidates.as_slice() {
        [(id, _)] => Ok(*id),
        [] => bail!("no mission has the title or id `{input}`; see `volee mission ls --all`"),
        several => bail!(
            "`{input}` could be {}; use the id",
            several
                .iter()
                .map(|(id, title)| format!("\"{}\" ({})", text::one_line(title), id.short()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The question waiting for the person that `input` names: its identifier,
/// its short identifier, or the agent that asked it, when that agent asked
/// only one.
pub(crate) fn question(registry: &Registry, input: &str) -> Result<MessageId> {
    let input = input.trim();
    let waiting = registry.unanswered_questions();
    if let Ok(id) = input.parse::<MessageId>() {
        if waiting.iter().any(|question| question.id == id) {
            return Ok(id);
        }
        if registry.message(id).is_some() {
            bail!("message {id} is not a question waiting for you; see `volee msg questions`");
        }
        bail!("volee knows no message {id}; see `volee msg questions`");
    }
    let by_id: Vec<&Message> = waiting
        .iter()
        .copied()
        .filter(|question| ends_with(&question.id.to_string(), MessageId::PREFIX, input))
        .collect();
    let candidates = if by_id.is_empty() {
        agent(registry, input).map_or_else(
            |_| Vec::new(),
            |asker| {
                waiting
                    .iter()
                    .copied()
                    .filter(|question| question.from == Party::Agent(asker))
                    .collect()
            },
        )
    } else {
        by_id
    };
    match candidates.as_slice() {
        [question] => Ok(question.id),
        [] => bail!(
            "no question waiting for you has the id `{input}` or comes from an agent of that \
             name; see `volee msg questions`"
        ),
        several => bail!(
            "`{input}` could be {}; use the id",
            several
                .iter()
                .map(|question| question.id.short())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Refuses to go on in the shell of an agent volee launched, for a command
/// that speaks for the person; `only` says what only the person does, such
/// as "answers a question asked to them".
///
/// An agent has a shell, and what the person types there the agent can
/// type too. This is a guard against a helpful model answering its own
/// question, not a lock: volee marks the shells of its agents, and an agent
/// that removes the mark has decided to.
pub(crate) fn as_the_person(only: &str) -> Result<()> {
    refuse_agent(
        std::env::var(volee_proto::paths::AGENT_ENV).ok().as_deref(),
        only,
    )
}

fn refuse_agent(agent: Option<&str>, only: &str) -> Result<()> {
    match agent.map(str::trim).filter(|agent| !agent.is_empty()) {
        None => Ok(()),
        Some(agent) => bail!(
            "this shell belongs to the agent {agent}, which volee launched: only the person \
             {only}. An agent writes with volee's MCP tools (send_message, ask_human, \
             request_approval), and waits for the person's answer."
        ),
    }
}

/// The configuration file, or the defaults when there is none.
pub(crate) fn load_config() -> Result<Config> {
    let file = paths()?.config_file();
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    Ok(Config::load(&file, &home)?)
}

/// A repository as the daemon understands it: a name from the
/// configuration, or the absolute path of a directory (resolved against
/// `cwd`).
pub(crate) fn repository(config: &Config, cwd: &Path, input: &str) -> Result<String> {
    if config.repos.contains_key(input) {
        return Ok(input.to_owned());
    }
    let path = cwd.join(input);
    if !path.is_dir() {
        bail!(
            "unknown repository `{input}`: it is not a [repos.<name>] of the configuration (see \
             `volee config check`) nor a directory"
        );
    }
    let path = path
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", path.display()))?;
    if let Some((name, _)) = config.repo_at(&path) {
        return Ok(name.to_owned());
    }
    path.into_os_string()
        .into_string()
        .map_err(|path| anyhow::anyhow!("{} is not valid UTF-8", Path::new(&path).display()))
}

/// Whether the identifier `full` ends with what the person typed, with or
/// without its prefix.
fn ends_with(full: &str, prefix: &str, input: &str) -> bool {
    let typed = input
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('_'))
        .unwrap_or(input)
        .to_lowercase();
    typed.len() >= MIN_SUFFIX && full.ends_with(&typed)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ulid::Ulid;
    use volee_core::harness::HarnessKind;
    use volee_core::message::Thread;
    use volee_core::mission::{Mission, MissionKind, MissionStatus};
    use volee_core::role::{LaunchedBy, Role};
    use volee_core::time::Timestamp;
    use volee_core::workspace::{Location, Workspace};
    use volee_core::{Event, EventKind, ThreadId};

    use super::*;

    #[test]
    fn the_person_s_commands_refuse_the_shell_of_an_agent() {
        assert!(refuse_agent(None, "answers a question asked to them").is_ok());
        assert!(refuse_agent(Some("  "), "answers a question asked to them").is_ok());
        let refusal = refuse_agent(Some("ag_01abc"), "answers a question asked to them")
            .unwrap_err()
            .to_string();
        assert!(
            refusal.contains("belongs to the agent ag_01abc")
                && refusal.contains("only the person answers a question asked to them")
                && refusal.contains("request_approval"),
            "{refusal}"
        );
    }

    fn agent_id(n: u128) -> AgentId {
        AgentId::from_ulid(Ulid::from_parts(1_000, n))
    }

    fn mission_id(n: u128) -> MissionId {
        MissionId::from_ulid(Ulid::from_parts(4_000, n))
    }

    fn world() -> Registry {
        let at = |kind| Event::new(Timestamp::from_millis(1), kind);
        let mut events = Vec::new();
        for (n, name) in [(1, "billing"), (2, "api"), (3, "api")] {
            events.push(at(EventKind::AgentRegistered {
                agent: agent_id(n),
                name: name.into(),
                role: Role::Lead,
                harness: HarnessKind::ClaudeCode,
                launched_by: LaunchedBy::Human,
                parent: None,
                mission: None,
                workspace: Workspace::at("/work"),
                location: Location::Background,
            }));
        }
        for (n, title) in [(1, "Invoicing"), (2, "Audit")] {
            let thread = ThreadId::from_ulid(Ulid::from_parts(2_000, n));
            events.push(at(EventKind::ThreadOpened {
                thread: Thread {
                    id: thread,
                    title: title.into(),
                    mission: Some(mission_id(n)),
                    participants: vec![],
                    opened_at: Timestamp::from_millis(1),
                },
            }));
            events.push(at(EventKind::MissionCreated {
                mission: Mission {
                    id: mission_id(n),
                    title: title.into(),
                    kind: MissionKind::Analysis,
                    goal: "Look".into(),
                    repos: vec!["billing".into()],
                    requested_by: None,
                    coordinator: None,
                    thread,
                    status: MissionStatus::Planning,
                    report: None,
                    created_at: Timestamp::from_millis(1),
                    ended_at: None,
                    assessments: BTreeMap::new(),
                },
            }));
        }
        Registry::from_events(&events).unwrap()
    }

    #[test]
    fn agents_resolve_by_id_short_id_and_name() {
        let registry = world();
        let id = agent_id(1);
        assert_eq!(agent(&registry, &id.to_string()).unwrap(), id);
        assert_eq!(agent(&registry, &id.short()).unwrap(), id);
        let bare = id.short().trim_start_matches("ag_").to_uppercase();
        assert_eq!(agent(&registry, &bare).unwrap(), id);
        assert_eq!(agent(&registry, "billing").unwrap(), id);
    }

    #[test]
    fn ambiguous_and_unknown_agents_are_explained() {
        let registry = world();
        let ambiguous = agent(&registry, "api").unwrap_err().to_string();
        assert!(ambiguous.contains("could be"), "{ambiguous}");
        assert!(agent(&registry, "nobody").is_err());
        // Too short to be an id ending: no accidental matches.
        assert!(agent(&registry, "1").is_err());
        let unknown = agent_id(9).to_string();
        assert!(agent(&registry, &unknown).is_err());
    }

    #[test]
    fn missions_resolve_by_id_short_id_and_title() {
        let registry = world();
        let id = mission_id(2);
        assert_eq!(mission(&registry, &id.to_string()).unwrap(), id);
        assert_eq!(mission(&registry, &id.short()).unwrap(), id);
        assert_eq!(mission(&registry, "audit").unwrap(), id);
        assert!(mission(&registry, "Nothing").is_err());
    }
}
