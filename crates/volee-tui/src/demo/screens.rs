//! Terminal screens for the demo's preview: a short transcript of each
//! agent's session, written as raw terminal output (ANSI colors and cursor
//! moves), so the preview goes through the same terminal emulator as real
//! sessions.

use unicode_width::UnicodeWidthStr;
use volee_core::Agent;
use volee_core::state::{AgentState, InputRequest};

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const ORANGE: &str = "\x1b[38;5;209m";
const BLUE: &str = "\x1b[38;5;111m";
const GRAY: &str = "\x1b[38;5;244m";

/// A session of the demo: its agent, what it was asked, and the tools it
/// used (name, argument, result).
struct Transcript {
    agent: &'static str,
    prompt: &'static str,
    steps: &'static [(&'static str, &'static str, &'static str)],
}

const TRANSCRIPTS: &[Transcript] = &[
    Transcript {
        agent: "tower",
        prompt: "Add invoicing to the product, and find out why search got slower last week.",
        steps: &[
            (
                "Bash",
                "volee mission start Invoicing --repos billing,mcp-server,dashboard",
                "Mission Invoicing started: coordinator invoicing",
            ),
            (
                "Bash",
                "volee mission start \"Latency audit\" --analysis --repos gateway,search",
                "Mission Latency audit started: coordinator latency-audit",
            ),
            ("Read", "docs/roadmap.md", "Read 88 lines"),
        ],
    },
    Transcript {
        agent: "invoicing",
        prompt: "Coordinate mission Invoicing across billing, mcp-server and dashboard.",
        steps: &[
            ("Write", "tasks/billing.md", "Wrote 31 lines"),
            (
                "Bash",
                "volee spawn --repo billing --task tasks/billing.md",
                "Worker invoice-schema started",
            ),
            (
                "Bash",
                "volee spawn --repo mcp-server --task tasks/mcp-server.md",
                "Worker invoice-tools started",
            ),
            (
                "Bash",
                "volee spawn --repo dashboard --task tasks/dashboard.md",
                "Worker invoice-page started",
            ),
            ("volee", "send_message contract", "Delivered to 3 workers"),
        ],
    },
    Transcript {
        agent: "invoice-schema",
        prompt: "Add the invoices table, its migration and the totals logic in billing.",
        steps: &[
            ("Read", "src/db/schema.ts", "Read 142 lines"),
            (
                "Update",
                "src/db/schema.ts",
                "Updated with 31 additions and 2 removals",
            ),
            (
                "Update",
                "src/invoices/totals.ts",
                "Updated with 64 additions",
            ),
            ("Bash", "bun test invoices", "12 pass, 0 fail"),
            ("Write", "migrations/0042_invoices.sql", "Wrote 48 lines"),
        ],
    },
    Transcript {
        agent: "invoice-tools",
        prompt: "Expose create_invoice and list_invoices as MCP tools in mcp-server.",
        steps: &[
            ("Read", "src/tools/index.ts", "Read 64 lines"),
            ("Read", "docs/contract.md", "Read 22 lines"),
            ("Write", "src/tools/list_invoices.ts", "Wrote 57 lines"),
            ("Bash", "bun test tools", "8 pass, 0 fail"),
        ],
    },
    Transcript {
        agent: "invoice-page",
        prompt: "Build the invoices page in dashboard: list, detail and PDF download.",
        steps: &[
            ("Write", "app/invoices/page.tsx", "Wrote 120 lines"),
            ("Write", "app/invoices/[id]/page.tsx", "Wrote 38 lines"),
            ("Bash", "bun run build", "Compiled in 4.2 s"),
            (
                "Bash",
                "gh pr create --fill",
                "https://git.example.com/dashboard/pull/42",
            ),
        ],
    },
    Transcript {
        agent: "latency-audit",
        prompt: "Coordinate mission Latency audit (analysis, read only): gateway and search.",
        steps: &[
            ("Write", "tasks/gateway.md", "Wrote 18 lines"),
            (
                "Bash",
                "volee spawn --repo gateway --read-only",
                "Worker trace-gateway started",
            ),
            (
                "Bash",
                "volee spawn --repo search --read-only",
                "Worker trace-search started",
            ),
        ],
    },
    Transcript {
        agent: "trace-gateway",
        prompt: "Find where the p95 latency of search requests grows in gateway. Read only.",
        steps: &[
            ("Grep", "\"p95\" src/", "14 matches in 5 files"),
            ("Read", "config/limits.toml", "Read 40 lines"),
            (
                "Task",
                "explore-handlers: map the request path",
                "Running in the background",
            ),
            ("Read", "src/middleware/auth.rs", "Read 211 lines"),
        ],
    },
    Transcript {
        agent: "trace-search",
        prompt: "Measure the p95 latency of search queries since 2026-10-30. Read only.",
        steps: &[
            ("Read", "src/query/plan.rs", "Read 380 lines"),
            (
                "Bash",
                "cargo bench --bench query",
                "p95 12.1 ms (was 11.8 ms)",
            ),
            ("Read", "benches/results.json", "Read 96 lines"),
        ],
    },
    Transcript {
        agent: "explore-handlers",
        prompt: "Map the request path of /search through the gateway handlers.",
        steps: &[
            ("Grep", "\"p95\" src/handlers", "3 matches"),
            ("Read", "src/handlers/search.rs", "Read 158 lines"),
        ],
    },
    Transcript {
        agent: "explore-traces",
        prompt: "Find the slowest spans in yesterday's traces.",
        steps: &[
            ("Read", "traces/2026-11-05.jsonl", "Read 2,000 lines"),
            ("Grep", "\"auth\" traces/", "418 matches"),
        ],
    },
    Transcript {
        agent: "broken-links",
        prompt: "Fix the broken links reported by the docs checker.",
        steps: &[
            ("Bash", "npm run check-links", "7 broken links in 4 files"),
            (
                "Update",
                "docs/guide/install.md",
                "Updated with 3 additions and 3 removals",
            ),
            (
                "Update",
                "docs/guide/links.md",
                "Updated with 4 additions and 4 removals",
            ),
        ],
    },
    Transcript {
        agent: "infra",
        prompt: "Explain what terraform plan would change in staging.",
        steps: &[
            ("Read", "main.tf", "Read 312 lines"),
            (
                "Bash",
                "terraform plan -out plan.bin",
                "Plan: 2 to add, 1 to change, 0 to destroy",
            ),
        ],
    },
];

/// The session of an agent of the demo.
fn transcript(name: &str) -> &'static Transcript {
    const UNKNOWN: Transcript = Transcript {
        agent: "",
        prompt: "Help me with this repository.",
        steps: &[],
    };
    TRANSCRIPTS
        .iter()
        .find(|transcript| transcript.agent == name)
        .unwrap_or(&UNKNOWN)
}

/// The screen of an agent in its current state.
pub(crate) fn screen(agent: &Agent) -> String {
    let transcript = transcript(&agent.name);
    let mut screen = Screen::default();
    // Start from a clean screen, like a terminal replay does.
    screen.raw("\x1b[2J\x1b[H");
    screen.line(&format!("{BOLD}>{RESET} {}", transcript.prompt));
    screen.line("");
    for (tool, argument, result) in transcript.steps {
        screen.line(&format!(
            "{GREEN}\u{23fa}{RESET} {BOLD}{tool}{RESET}({argument})"
        ));
        screen.line(&format!("  {GRAY}\u{23bf}{RESET}  {DIM}{result}{RESET}"));
        screen.line("");
    }
    match &agent.state {
        AgentState::NeedsInput {
            request: InputRequest::Permission { .. },
        } => {
            let command = agent
                .detail
                .as_deref()
                .and_then(|detail| detail.split_once(": "))
                .map_or("the next command", |(_, command)| command);
            screen.boxed(
                BLUE,
                &[
                    format!("{BOLD}Bash command{RESET}"),
                    String::new(),
                    format!("  {command}"),
                    format!("  {DIM}Apply the pending migrations to the local database{RESET}"),
                    String::new(),
                    "Do you want to proceed?".to_owned(),
                    format!("{BLUE}\u{276f}{RESET} 1. Yes"),
                    format!("  2. Yes, and don't ask again for {command}"),
                    "  3. No, and tell Claude what to do differently (esc)".to_owned(),
                ],
            );
        }
        AgentState::NeedsInput { .. } => {
            let question = agent
                .detail
                .as_deref()
                .unwrap_or("One question before I go on.");
            screen.line(&format!(
                "{GREEN}\u{23fa}{RESET} Before I go on, one question:"
            ));
            screen.line(&format!("  {BOLD}{question}{RESET}"));
            screen.line(&format!("  {DIM}I also sent it to your inbox.{RESET}"));
            screen.line("");
            screen.prompt();
        }
        AgentState::Working | AgentState::Launching => {
            let doing = agent
                .last_activity
                .as_ref()
                .map_or("Thinking", |activity| activity.summary.as_str());
            screen.line(&format!(
                "{ORANGE}\u{273b}{RESET} {doing}\u{2026} {DIM}(esc to interrupt){RESET}"
            ));
            screen.line("");
            screen.prompt();
        }
        AgentState::Done => {
            let summary = agent.detail.as_deref().unwrap_or("Done.");
            screen.line(&format!("{GREEN}\u{23fa}{RESET} {summary}"));
            screen.line("");
            screen.prompt();
        }
        _ => screen.prompt(),
    }
    screen.text
}

#[derive(Default)]
struct Screen {
    text: String,
}

impl Screen {
    fn raw(&mut self, bytes: &str) {
        self.text.push_str(bytes);
    }

    fn line(&mut self, line: &str) {
        self.text.push_str(line);
        self.text.push_str("\r\n");
    }

    /// Lines in a rounded box; `lines` may hold color codes.
    fn boxed(&mut self, color: &str, lines: &[String]) {
        let width = 74;
        self.line(&format!(
            "{color}\u{256d}{}\u{256e}{RESET}",
            "\u{2500}".repeat(width)
        ));
        for line in lines {
            let visible = strip_ansi(line).width();
            let padding = " ".repeat(width.saturating_sub(visible + 1));
            self.line(&format!(
                "{color}\u{2502}{RESET} {line}{padding}{color}\u{2502}{RESET}"
            ));
        }
        self.line(&format!(
            "{color}\u{2570}{}\u{256f}{RESET}",
            "\u{2500}".repeat(width)
        ));
    }

    /// The input box of an idle session.
    fn prompt(&mut self) {
        self.boxed(GRAY, &[format!("{BOLD}>{RESET}")]);
        self.line(&format!("  {DIM}? for shortcuts{RESET}"));
    }
}

/// The text of a line without its escape sequences.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_sequences_are_not_counted_as_text() {
        assert_eq!(strip_ansi("\x1b[1mBash\x1b[0m(x)"), "Bash(x)");
        assert_eq!(strip_ansi("plain"), "plain");
    }
}
