//! Which process a hook comes from (ADR 0026).
//!
//! volee gives each agent its hooks through `claude --settings`, with the
//! agent's id on the command line. Another session can inherit them: Claude
//! Code's own agents view, opened inside a hosted agent, revives or forks a
//! background session with the same settings. Its hooks then name the
//! hosted agent, and the daemon took that session for the agent's: the
//! agent moved to the background and its messages went to the other
//! session. For an agent in a running volee host, a hook counts only when
//! it comes from the host's program or one of its descendants.

use std::collections::HashMap;
use std::process::Command;
use std::sync::Arc;

use volee_core::AgentId;

use crate::server::Shared;

/// Whether a hook that `pid` ran for `agent` comes from another program
/// than the agent's: the agent runs in a volee host whose program is known,
/// and `pid` is neither that program nor one of its descendants. Unknown
/// cases (no pid, no host, no process table) count as the agent's own.
pub(crate) async fn foreign(shared: &Arc<Shared>, agent: AgentId, pid: Option<u32>) -> bool {
    let Some(pid) = pid else {
        return false;
    };
    let host = {
        let state = shared.state.lock().await;
        state
            .registry()
            .agent(agent)
            .and_then(|record| record.location.host().map(str::to_owned))
    };
    let Some(host) = host else {
        return false;
    };
    if !crate::launch::host_runs(shared, &host) {
        return false;
    }
    let record = crate::launch::hosts_dir(shared).join(format!("{host}.json"));
    let Some(program) = volee_host::HostRecord::read(&record)
        .ok()
        .and_then(|record| record.child_pid)
    else {
        return false;
    };
    if pid == program {
        return false;
    }
    let parents = tokio::task::spawn_blocking(process_parents)
        .await
        .ok()
        .flatten();
    let Some(parents) = parents else {
        return false;
    };
    let ours = descends(pid, program, &parents);
    if !ours {
        tracing::info!(
            %agent,
            pid,
            program,
            "a hook from another program than the agent's volee host, ignored"
        );
    }
    !ours
}

/// Every process's parent, from `ps`; `None` when it cannot run.
fn process_parents() -> Option<HashMap<u32, u32>> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid="])
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    Some(parents(&String::from_utf8_lossy(&output.stdout)))
}

/// Parses `ps -axo pid=,ppid=`.
fn parents(output: &str) -> HashMap<u32, u32> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let ppid = fields.next()?.parse().ok()?;
            Some((pid, ppid))
        })
        .collect()
}

/// Whether `pid` is `ancestor` or one of its descendants.
fn descends(pid: u32, ancestor: u32, parents: &HashMap<u32, u32>) -> bool {
    let mut current = pid;
    // Deep enough for any process tree; a loop in a racing table ends too.
    for _ in 0..64 {
        if current == ancestor {
            return true;
        }
        match parents.get(&current) {
            Some(&parent) if parent > 1 && parent != current => current = parent,
            _ => return false,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descendants_are_found_through_their_parents() {
        let table = parents("  1     0\n 100     1\n 200   100\n 300   200\n 400     1\n junk\n");
        assert_eq!(table.len(), 5);
        assert!(descends(300, 100, &table), "a grandchild");
        assert!(descends(200, 200, &table), "the program itself");
        assert!(
            !descends(400, 100, &table),
            "a session started by someone else"
        );
        assert!(!descends(999, 100, &table), "a process that is gone");
        assert!(
            !descends(100, 300, &table),
            "an ancestor is not a descendant"
        );
    }
}
