//! The `volee` command.
//!
//! A command-line program talks to its user through stdout and stderr, so
//! printing is allowed in this crate and only here.
#![allow(clippy::print_stdout, clippy::print_stderr)]

mod cmd;
mod connect;
mod table;

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use volee_core::AgentId;
use volee_core::message::MessageKind;

/// See, launch and coordinate every coding agent on your machine.
#[derive(Debug, Parser)]
#[command(name = "volee", version = volee_core::VERSION, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start, stop or inspect the background daemon.
    #[command(subcommand)]
    Daemon(DaemonCommand),
    /// List every agent, the ones that need you first.
    Ls {
        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Send messages to agents.
    #[command(subcommand)]
    Msg(MsgCommand),
    /// Serve the MCP tools of an agent over stdio. Harnesses start this.
    Mcp {
        /// The agent the server speaks for.
        #[arg(long)]
        agent: AgentId,
    },
    /// Called by the hooks of agents volee launched. Not for people.
    #[command(hide = true)]
    Hook {
        /// The hook event in kebab case, or `wait`.
        event: String,
        /// The agent the hook belongs to.
        #[arg(long)]
        agent: AgentId,
        /// For `wait`: how long to wait for a message, in seconds.
        #[arg(long, default_value_t = 3_600)]
        timeout: u64,
    },
}

#[derive(Debug, Subcommand)]
enum DaemonCommand {
    /// Run the daemon in the foreground.
    Run,
    /// Start the daemon in the background, if it is not running.
    Start,
    /// Stop the running daemon.
    Stop,
    /// Show whether the daemon runs, and its counters.
    Status,
}

#[derive(Debug, Subcommand)]
enum MsgCommand {
    /// Send a message from you to an agent.
    Send {
        /// The recipient agent.
        to: AgentId,
        /// The message.
        body: String,
        /// What the message is for.
        #[arg(long, value_enum, default_value_t = Kind::Note)]
        kind: Kind,
    },
}

/// Message kinds a person sends from the command line.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum Kind {
    Note,
    Question,
    Answer,
    Contract,
    Alert,
}

impl From<Kind> for MessageKind {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::Note => Self::Note,
            Kind::Question => Self::Question,
            Kind::Answer => Self::Answer,
            Kind::Contract => Self::Contract,
            Kind::Alert => Self::Alert,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        // Hooks run dozens of times per turn: no async runtime, no logging setup.
        Some(Command::Hook {
            event,
            agent,
            timeout,
        }) => cmd::hook::run(&event, agent, timeout),
        Some(command) => async_main(command),
        None => {
            println!(
                "volee {} is under construction: try `volee ls`, or see the roadmap at \
                 https://volee-org.github.io/volee/project/roadmap.html",
                volee_core::VERSION
            );
            ExitCode::SUCCESS
        }
    }
}

fn async_main(command: Command) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("volee: cannot start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(async {
        match command {
            Command::Daemon(DaemonCommand::Run) => cmd::daemon::run().await,
            Command::Daemon(DaemonCommand::Start) => cmd::daemon::start().await,
            Command::Daemon(DaemonCommand::Stop) => cmd::daemon::stop().await,
            Command::Daemon(DaemonCommand::Status) => cmd::daemon::status().await,
            Command::Ls { json } => cmd::ls::run(json).await,
            Command::Msg(MsgCommand::Send { to, body, kind }) => {
                cmd::msg::send(to, body, kind.into()).await
            }
            Command::Mcp { agent } => cmd::mcp::run(agent).await,
            Command::Hook { .. } => unreachable!("hooks run without the async runtime"),
        }
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("volee: {error:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
