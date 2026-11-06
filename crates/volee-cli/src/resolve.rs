//! Finding the agent or mission a person means: a full identifier, the
//! short form tables print (`ag_x7k2pq`), or a name.

use anyhow::{Result, bail};
use volee_core::{AgentId, MissionId, Registry};

/// The shortest identifier ending accepted, so that a typo rarely matches.
const MIN_SUFFIX: usize = 4;

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
                .map(|(id, name)| format!("{name} ({})", id.short()))
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
                .map(|(id, title)| format!("\"{title}\" ({})", id.short()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
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
