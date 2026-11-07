//! The `volee` command.
//!
//! A command-line program talks to its user through stdout and stderr, so
//! printing is allowed in this crate and only here.
#![allow(clippy::print_stdout, clippy::print_stderr)]

mod cmd;
mod connect;
mod resolve;
mod table;

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use volee_core::AgentId;
use volee_core::message::MessageKind;
use volee_core::mission::MissionKind;

/// See, launch and coordinate every coding agent on your machine. Without a
/// command, opens the interface.
#[derive(Debug, Parser)]
#[command(name = "volee", version = volee_core::VERSION, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show, create or check the configuration file.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Start, stop or inspect the background daemon.
    #[command(subcommand)]
    Daemon(DaemonCommand),
    /// List every agent, the ones that need you first.
    Ls {
        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Launch an agent on a new branch, in its own worktree.
    Spawn {
        /// The repository: a name from the configuration, or a directory.
        repo: String,
        /// The branch to create for the agent.
        branch: String,
        /// The task, as the agent's first prompt.
        task: String,
        /// The branch to start from; the repository's default branch when
        /// absent.
        #[arg(long)]
        base: Option<String>,
        /// The model; the configured one, else Claude Code's default.
        #[arg(long)]
        model: Option<String>,
        /// A display name; derived from the repository and branch when
        /// absent.
        #[arg(long)]
        name: Option<String>,
    },
    /// Take an agent over in this terminal; Ctrl+Z comes back.
    Attach {
        /// The agent: its name, short id or id.
        agent: String,
    },
    /// Print an agent's recent terminal output.
    Logs {
        /// The agent: its name, short id or id.
        agent: String,
    },
    /// Show an agent's changes against the branch it started from.
    Diff {
        /// The agent: its name, short id or id.
        agent: String,
        /// The full patch instead of a summary by file.
        #[arg(long)]
        full: bool,
    },
    /// Stop an agent volee launched; its conversation is kept.
    Stop {
        /// The agent: its name, short id or id.
        agent: String,
    },
    /// Remove the worktree of an agent whose work landed; its branch is kept.
    Tidy {
        /// The agent: its name, short id or id.
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        agent: Option<String>,
        /// Every agent volee launched whose evidence allows it.
        #[arg(long)]
        all: bool,
        /// Only say what would be removed and what kept.
        #[arg(long)]
        dry_run: bool,
    },
    /// Send messages to agents.
    #[command(subcommand)]
    Msg(MsgCommand),
    /// Summarize a mission's thread, or your thread with an agent, with a
    /// model. Off unless `summaries = true` under `[messages]`.
    Summary {
        /// A mission (title, short id or id) or an agent (name, short id or
        /// id).
        target: String,
    },
    /// Start, follow, abort or close missions across repositories.
    #[command(subcommand)]
    Mission(MissionCommand),
    /// Serve the MCP tools of an agent over stdio. Harnesses start this.
    Mcp {
        /// The agent the server speaks for.
        #[arg(long)]
        agent: AgentId,
    },
    /// Open the interface on a simulated team of agents, to try volee.
    Demo {
        /// The theme, instead of the one in the configuration: a built-in
        /// one or a file in ~/.config/volee/themes.
        #[arg(long)]
        theme: Option<String>,
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
enum ConfigCommand {
    /// Print the path of the configuration file.
    Path,
    /// Write a commented example, unless the file exists.
    Init,
    /// Check the file and summarize what it sets.
    Check,
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
        /// The recipient: an agent's name, its short id (as `volee ls` prints it) or its id.
        to: String,
        /// The message.
        body: String,
        /// What the message is for.
        #[arg(long, value_enum, default_value_t = Kind::Note)]
        kind: Kind,
    },
}

#[derive(Debug, Subcommand)]
enum MissionCommand {
    /// Start a mission: one goal, carried out across one or more repositories.
    New {
        /// A short title.
        title: String,
        /// The goal, as the coordinator will read it.
        goal: String,
        /// A repository: a name from the configuration, or a directory.
        /// Repeat the option or separate names with commas.
        #[arg(long = "repo", short, required = true, value_delimiter = ',')]
        repos: Vec<String>,
        /// What the mission delivers.
        #[arg(long, value_enum, default_value_t = MissionKindArg::Change)]
        kind: MissionKindArg,
    },
    /// List open missions.
    Ls {
        /// Include closed and aborted missions.
        #[arg(long)]
        all: bool,
        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Show a mission: its agents, the verdicts on its workers, and its thread.
    Show {
        /// The mission: its title, short id or id.
        mission: String,
    },
    /// Abort a mission. Its agents are told to stop; nothing is deleted.
    Abort {
        /// The mission: its title, short id or id.
        mission: String,
        /// Why, told to the agents.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Close a mission once every worker delivered.
    Close {
        /// The mission: its title, short id or id.
        mission: String,
        /// Close it even if some workers have not delivered.
        #[arg(long)]
        force: bool,
    },
}

/// What a mission delivers.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum MissionKindArg {
    /// Branches and pull requests.
    Change,
    /// Reports.
    Analysis,
}

impl From<MissionKindArg> for MissionKind {
    fn from(kind: MissionKindArg) -> Self {
        match kind {
            MissionKindArg::Change => Self::Change,
            MissionKindArg::Analysis => Self::Analysis,
        }
    }
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
        Some(Command::Config(command)) => report(match command {
            ConfigCommand::Path => cmd::config::path(),
            ConfigCommand::Init => cmd::config::init(),
            ConfigCommand::Check => cmd::config::check(),
        }),
        Some(command) => async_main(command),
        None => block_on(cmd::interface::run()),
    }
}

/// Runs a command on a multi-threaded runtime and reports how it ended.
fn block_on(command: impl Future<Output = anyhow::Result<()>>) -> ExitCode {
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
    report(runtime.block_on(command))
}

fn async_main(command: Command) -> ExitCode {
    block_on(async {
        match command {
            Command::Daemon(DaemonCommand::Run) => cmd::daemon::run().await,
            Command::Daemon(DaemonCommand::Start) => cmd::daemon::start().await,
            Command::Daemon(DaemonCommand::Stop) => cmd::daemon::stop().await,
            Command::Daemon(DaemonCommand::Status) => cmd::daemon::status().await,
            Command::Ls { json } => cmd::ls::run(json).await,
            Command::Attach { agent } => cmd::agent::attach(&agent).await,
            Command::Logs { agent } => cmd::agent::logs(&agent).await,
            Command::Diff { agent, full } => cmd::agent::diff(&agent, full).await,
            Command::Stop { agent } => cmd::agent::stop(&agent).await,
            Command::Tidy {
                agent,
                all,
                dry_run,
            } => cmd::tidy::run(agent.as_deref(), all, dry_run).await,
            Command::Msg(MsgCommand::Send { to, body, kind }) => {
                cmd::msg::send(&to, body, kind.into()).await
            }
            Command::Mission(command) => mission(command).await,
            Command::Summary { target } => cmd::summary::run(&target).await,
            Command::Spawn {
                repo,
                branch,
                task,
                base,
                model,
                name,
            } => {
                cmd::spawn::run(cmd::spawn::SpawnRequest {
                    repo,
                    branch,
                    task,
                    base,
                    model,
                    name,
                })
                .await
            }
            Command::Mcp { agent } => cmd::mcp::run(agent).await,
            Command::Demo { theme } => cmd::demo::run(theme.as_deref()).await,
            Command::Hook { .. } | Command::Config(_) => {
                unreachable!("these commands run without the async runtime")
            }
        }
    })
}

async fn mission(command: MissionCommand) -> anyhow::Result<()> {
    match command {
        MissionCommand::New {
            title,
            goal,
            repos,
            kind,
        } => {
            cmd::mission::new(cmd::mission::NewMission {
                title,
                goal,
                kind: kind.into(),
                repos,
            })
            .await
        }
        MissionCommand::Ls { all, json } => cmd::mission::list(all, json).await,
        MissionCommand::Show { mission } => cmd::mission::show(&mission).await,
        MissionCommand::Abort { mission, reason } => cmd::mission::abort(&mission, reason).await,
        MissionCommand::Close { mission, force } => cmd::mission::close(&mission, force).await,
    }
}

fn report(result: anyhow::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // A refusal from the daemon is meant for people: its code is not.
            match error.downcast_ref::<volee_proto::client::ClientError>() {
                Some(volee_proto::client::ClientError::Rpc(refusal)) => {
                    eprintln!("volee: {}", refusal.message);
                }
                _ => eprintln!("volee: {error:#}"),
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command};
    use clap::{CommandFactory, Parser};

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn demo_takes_an_optional_theme() {
        let cli = Cli::try_parse_from(["volee", "demo", "--theme", "terminal"]).unwrap();
        assert!(
            matches!(cli.command, Some(Command::Demo { theme: Some(ref t) }) if t == "terminal")
        );
        let cli = Cli::try_parse_from(["volee", "demo"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Demo { theme: None })));
    }

    #[test]
    fn no_command_opens_the_interface() {
        let cli = Cli::try_parse_from(["volee"]).unwrap();
        assert!(cli.command.is_none());
    }

    fn parse(args: &[&str]) -> Command {
        Cli::try_parse_from(args).unwrap().command.unwrap()
    }

    #[test]
    fn agent_commands_take_an_agent_as_people_type_it() {
        assert!(matches!(
            parse(&["volee", "attach", "alpha-feat-x"]),
            Command::Attach { agent } if agent == "alpha-feat-x"
        ));
        assert!(matches!(
            parse(&["volee", "logs", "ag_x7k2pq"]),
            Command::Logs { agent } if agent == "ag_x7k2pq"
        ));
        assert!(matches!(
            parse(&["volee", "diff", "alpha"]),
            Command::Diff { agent, full: false } if agent == "alpha"
        ));
        assert!(matches!(
            parse(&["volee", "diff", "alpha", "--full"]),
            Command::Diff { full: true, .. }
        ));
        assert!(matches!(
            parse(&["volee", "stop", "alpha"]),
            Command::Stop { agent } if agent == "alpha"
        ));
        assert!(Cli::try_parse_from(["volee", "attach"]).is_err());
    }

    #[test]
    fn tidy_takes_one_agent_or_all_of_them() {
        assert!(matches!(
            parse(&["volee", "tidy", "alpha"]),
            Command::Tidy { agent: Some(agent), all: false, dry_run: false } if agent == "alpha"
        ));
        assert!(matches!(
            parse(&["volee", "tidy", "--all", "--dry-run"]),
            Command::Tidy {
                agent: None,
                all: true,
                dry_run: true
            }
        ));
        assert!(Cli::try_parse_from(["volee", "tidy"]).is_err());
        assert!(Cli::try_parse_from(["volee", "tidy", "alpha", "--all"]).is_err());
        assert!(
            Cli::try_parse_from(["volee", "tidy", "alpha", "--force"]).is_err(),
            "there is no way to force a removal"
        );
    }
}
