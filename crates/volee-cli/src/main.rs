//! The `volee` command.
//!
//! A command-line program talks to its user through stdout and stderr, so
//! printing is allowed in this crate and only here.
#![allow(clippy::print_stdout, clippy::print_stderr)]

mod cmd;
mod connect;
mod resolve;
mod table;
mod text;
mod tools;

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
    /// Print an agent's screen as it looks now, as plain text.
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
    /// Stop listing an agent that no longer runs; its sessions, worktree and
    /// branch stay as they are.
    Forget {
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
    /// Serve the MCP tools of an agent over stdio. Harnesses start this:
    /// with --agent for the agents volee launches, with --join to bring an
    /// agent of another harness (Codex CLI, Gemini CLI...) into volee.
    Mcp {
        /// The agent the server speaks for.
        #[arg(long, required_unless_present = "join", conflicts_with = "join")]
        agent: Option<AgentId>,
        /// Join volee as an agent of another harness, under this name. The
        /// same name, harness and directory find the same agent again.
        #[arg(long, value_name = "NAME")]
        join: Option<String>,
        /// With --join: the harness it runs in, such as codex or gemini-cli.
        #[arg(long, requires = "join", default_value = "other")]
        harness: String,
        /// With --join: an open mission to take part in (its title, short id
        /// or id), so that its members and this agent can write to each
        /// other.
        #[arg(long, requires = "join", conflicts_with = "agent")]
        mission: Option<String>,
    },
    /// Print the completion script of a shell (bash, elvish, fish,
    /// powershell or zsh); the installation guide says where to put it.
    Completions {
        /// The shell.
        shell: clap_complete::Shell,
    },
    /// Open the interface on a simulated team of agents, to try volee.
    Demo {
        /// The theme, instead of the one in the configuration: a built-in
        /// one or a file in ~/.config/volee/themes.
        #[arg(long)]
        theme: Option<String>,
    },
    /// Run programs in hosts, watch them on a wall, take them over
    /// (prototype of ADR 0019).
    #[command(subcommand)]
    Host(HostCommand),
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
enum HostCommand {
    /// Start a program in a new host, detached from this terminal, and print
    /// the host's id.
    Spawn {
        /// The host's id; a new agent id when absent.
        #[arg(long)]
        id: Option<String>,
        /// The program's directory; this one when absent.
        #[arg(long)]
        cwd: Option<std::path::PathBuf>,
        /// Columns before anyone takes it over; this terminal's when absent.
        #[arg(long, requires = "rows")]
        cols: Option<u16>,
        /// Rows before anyone takes it over; this terminal's when absent.
        #[arg(long, requires = "cols")]
        rows: Option<u16>,
        /// The harness profile (ADR 0022).
        #[arg(long, default_value = "generic")]
        profile: String,
        /// Seconds the last screen stays after the program ends.
        #[arg(long, default_value_t = 600)]
        linger: u64,
        /// The command line, after `--`.
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// List hosts, running or ended.
    Ls {
        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Print a host's screen as plain text.
    Peek {
        /// The host's id.
        id: String,
    },
    /// Take a host over in this terminal; Ctrl+Q comes back.
    Attach {
        /// The host's id.
        id: String,
    },
    /// Ask a host to end its program.
    Stop {
        /// The host's id.
        id: String,
    },
    /// Show every running host as a wall of live screens.
    Wall,
    /// Run a host in this process. `spawn` starts this; not for people.
    #[command(hide = true)]
    Run {
        #[arg(long)]
        id: String,
        #[arg(long)]
        cwd: std::path::PathBuf,
        #[arg(long)]
        cols: u16,
        #[arg(long)]
        rows: u16,
        #[arg(long)]
        profile: String,
        #[arg(long)]
        linger: u64,
        #[arg(last = true, required = true)]
        command: Vec<String>,
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
        // Hosts run without the async runtime: a host is plain threads.
        Some(Command::Host(command)) => report(host(command)),
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
            Command::Forget { agent } => cmd::agent::forget(&agent).await,
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
            Command::Mcp {
                agent,
                join,
                harness,
                mission,
            } => match (agent, join) {
                (Some(agent), _) => cmd::mcp::run(agent).await,
                (None, Some(name)) => cmd::mcp::join(&name, &harness, mission.as_deref()).await,
                (None, None) => unreachable!("clap requires --agent or --join"),
            },
            Command::Completions { shell } => match completions(shell, &mut std::io::stdout()) {
                // The reader stopped early (`| head`): nothing to report.
                Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
                result => result.map_err(Into::into),
            },
            Command::Demo { theme } => cmd::demo::run(theme.as_deref()).await,
            Command::Hook { .. } | Command::Config(_) | Command::Host(_) => {
                unreachable!("these commands run without the async runtime")
            }
        }
    })
}

fn host(command: HostCommand) -> anyhow::Result<()> {
    match command {
        HostCommand::Spawn {
            id,
            cwd,
            cols,
            rows,
            profile,
            linger,
            command,
        } => cmd::host::spawn(cmd::host::Spawn {
            id,
            cwd,
            size: cols.zip(rows),
            profile,
            linger,
            command,
        }),
        HostCommand::Ls { json } => cmd::host::list(json),
        HostCommand::Peek { id } => cmd::host::peek(&id),
        HostCommand::Attach { id } => cmd::host::attach(&id),
        HostCommand::Stop { id } => cmd::host::stop(&id),
        HostCommand::Wall => cmd::host::wall(),
        HostCommand::Run {
            id,
            cwd,
            cols,
            rows,
            profile,
            linger,
            command,
        } => cmd::host::run(volee_host::HostConfig {
            id,
            dir: cmd::host::host_dir()?,
            command,
            cwd,
            env: Vec::new(),
            size: volee_host::Size { cols, rows },
            profile,
            scrollback: 1_000,
            linger: std::time::Duration::from_secs(linger),
        }),
    }
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
            // Both texts may quote names and messages that agents wrote.
            match error.downcast_ref::<volee_proto::client::ClientError>() {
                Some(volee_proto::client::ClientError::Rpc(refusal)) => {
                    eprintln!("volee: {}", text::printable(&refusal.message));
                }
                _ => eprintln!("volee: {}", text::printable(&format!("{error:#}"))),
            }
            ExitCode::FAILURE
        }
    }
}

/// Writes the completion script of `shell` for every command and option.
/// It is built in memory first: `clap_complete` panics on a failed write,
/// such as a pipe closed by `head`.
fn completions(shell: clap_complete::Shell, out: &mut dyn std::io::Write) -> std::io::Result<()> {
    let mut command = <Cli as clap::CommandFactory>::command();
    let mut script = Vec::new();
    clap_complete::generate(shell, &mut command, "volee", &mut script);
    out.write_all(&script)
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
    fn completions_cover_the_commands() {
        let cli = Cli::try_parse_from(["volee", "completions", "fish"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Completions {
                shell: clap_complete::Shell::Fish
            })
        ));
        let mut script = Vec::new();
        super::completions(clap_complete::Shell::Fish, &mut script).unwrap();
        let script = String::from_utf8(script).unwrap();
        for command in ["spawn", "mission", "forget", "completions"] {
            assert!(script.contains(command), "{command}");
        }
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

    #[test]
    fn mcp_serves_an_agent_volee_knows_or_joins_one() {
        let id = "ag_01jc5x8m3kq7v2c4b9e0f1g2h3";
        assert!(matches!(
            parse(&["volee", "mcp", "--agent", id]),
            Command::Mcp {
                agent: Some(_),
                join: None,
                ..
            }
        ));
        assert!(matches!(
            parse(&["volee", "mcp", "--join", "infra"]),
            Command::Mcp { agent: None, join: Some(name), harness, mission: None }
                if name == "infra" && harness == "other"
        ));
        assert!(matches!(
            parse(&[
                "volee", "mcp", "--join", "docs", "--harness", "gemini-cli", "--mission", "Invoicing",
            ]),
            Command::Mcp { join: Some(_), harness, mission: Some(mission), .. }
                if harness == "gemini-cli" && mission == "Invoicing"
        ));
        assert!(
            Cli::try_parse_from(["volee", "mcp"]).is_err(),
            "one or the other"
        );
        assert!(Cli::try_parse_from(["volee", "mcp", "--agent", id, "--join", "x"]).is_err());
        assert!(
            Cli::try_parse_from(["volee", "mcp", "--agent", id, "--mission", "x"]).is_err(),
            "only a joined agent picks its mission"
        );
    }
}
