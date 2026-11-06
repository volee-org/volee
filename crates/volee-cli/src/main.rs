//! The `volee` command.
//!
//! A command-line program talks to its user through stdout, so printing is
//! allowed here and only here.
#![allow(clippy::print_stdout)]

use clap::Parser;

/// See, launch and coordinate every coding agent on your machine.
#[derive(Debug, Parser)]
#[command(name = "volee", version = volee_core::VERSION, about, long_about = None)]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
    println!(
        "volee {} is under construction: see the roadmap at \
         https://volee-org.github.io/volee/project/roadmap.html",
        volee_core::VERSION
    );
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
