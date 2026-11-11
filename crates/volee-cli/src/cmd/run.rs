//! `volee run`: any program as an agent of the list, in this terminal
//! (ADR 0029).

use std::io::IsTerminal;
use std::path::Path;

use anyhow::{Context, Result, bail};
use volee_proto::client::ClientError;
use volee_proto::methods::{Run, RunParams};
use volee_proto::rpc;

use crate::cmd::host;
use crate::connect::connect_or_start;
use crate::text;

/// Seconds the program's last screen stays once it ended, as for any host.
const LINGER: u64 = 600;

/// Runs `command` in a volee host started here, so that it has this
/// directory and this terminal's environment, makes it an agent of the
/// list, and takes it over in this terminal when there is one. The leave key
/// comes back to the shell; the program keeps running.
pub(crate) async fn run(name: Option<String>, command: Vec<String>) -> Result<()> {
    // The daemon first: a daemon that cannot take the agent must say so
    // before anything is started.
    let client = connect_or_start().await?;
    let cwd = std::env::current_dir().context("cannot read the current directory")?;
    let name = name
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .or_else(|| folder(&cwd));
    let program = command
        .first()
        .map(|program| text::one_line(program).into_owned())
        .unwrap_or_default();
    let dir = host::host_dir()?;
    let spawn = host::Spawn {
        id: None,
        cwd: Some(cwd),
        size: None,
        profile: "generic".to_owned(),
        name: name.clone(),
        linger: LINGER,
        idle_exit: None,
        command,
    };
    let (dir, id) = tokio::task::spawn_blocking(move || {
        let id = host::start(&dir, spawn)?;
        anyhow::Ok((dir, id))
    })
    .await
    .context("the task that starts the program failed")??;
    let ran = client
        .call::<Run>(RunParams {
            host: id.clone(),
            name,
        })
        .await;
    let ran = match ran {
        Ok(ran) => ran,
        Err(error) => {
            // No list shows it: it must not keep running unseen.
            host::end(&dir, &id);
            bail!("{}", not_taken(&program, &error));
        }
    };
    drop(client);
    let name = text::one_line(&ran.name).into_owned();
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        println!("{name} runs under volee: `volee attach {name}` opens it.");
        return Ok(());
    }
    tokio::task::spawn_blocking(move || host::attach(&dir, &id))
        .await
        .context("the task that takes the program over failed")?
}

/// What to say when the daemon did not make an agent of the program this
/// command had started, and ended for that reason.
fn not_taken(program: &str, error: &ClientError) -> String {
    let why = match error {
        // A daemon from before `volee run`: it keeps running across an
        // upgrade until someone stops it.
        ClientError::Rpc(refusal) if refusal.code == rpc::METHOD_NOT_FOUND => {
            "the running daemon is older than this volee: stop it with `volee daemon stop` \
             and run the command again"
                .to_owned()
        }
        ClientError::Rpc(refusal) => refusal.message.clone(),
        other => other.to_string(),
    };
    format!(
        "{program} was started, then ended, since the daemon did not take it as an agent: {why}"
    )
}

/// The name of a directory, for an agent that was given none.
fn folder(cwd: &Path) -> Option<String> {
    cwd.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_says_that_the_program_was_ended_and_why() {
        let refusal = |code, message: &str| {
            ClientError::Rpc(rpc::RpcError {
                code,
                message: message.to_owned(),
                data: None,
            })
        };
        let said = not_taken(
            "codex",
            &refusal(rpc::REFUSED, "the volee host h1 already runs an agent"),
        );
        assert!(said.starts_with("codex was started, then ended"), "{said}");
        assert!(
            said.ends_with("the volee host h1 already runs an agent"),
            "{said}"
        );
        // A daemon that does not know the method is an older one.
        let old = not_taken(
            "codex",
            &refusal(rpc::METHOD_NOT_FOUND, "unknown method agents.run"),
        );
        assert!(old.contains("older than this volee"), "{old}");
        assert!(old.contains("`volee daemon stop`"), "{old}");
        assert!(!old.contains("unknown method"), "{old}");
    }

    #[test]
    fn an_agent_is_named_after_its_directory() {
        assert_eq!(
            folder(Path::new("/work/billing")).as_deref(),
            Some("billing")
        );
        assert_eq!(folder(Path::new("/")), None);
    }
}
