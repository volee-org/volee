//! The machine's process table, as far as volee reads it: which process
//! started which.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::error::HarnessError;
use crate::runner::{CommandRunner, Invocation, RunError};

/// `roots` and every process below them, from one `ps`.
///
/// A program started through a wrapper (`npx`, a shell, a version manager)
/// is not the process volee started but one of its descendants; whoever
/// looks for "the program of this terminal" in a list of processes needs
/// them all. Without `ps` the roots are all that can be said.
pub fn with_descendants(
    runner: &dyn CommandRunner,
    roots: &[u32],
) -> Result<HashSet<u32>, HarnessError> {
    let roots: HashSet<u32> = roots.iter().copied().collect();
    if roots.is_empty() {
        return Ok(roots);
    }
    // The same command line as the search for agents in the process table,
    // so that one table answers both.
    let ps = Invocation::new("ps", Duration::from_secs(5)).args(["-axo", "pid=,ppid=,args="]);
    let output = match runner.run(&ps) {
        Ok(output) => output,
        Err(RunError::NotFound { .. }) => return Ok(roots),
        Err(error) => return Err(HarnessError::Run(error)),
    };
    let parents = parents(&String::from_utf8_lossy(&output.stdout));
    Ok(below(&roots, &parents))
}

/// The parent of every process of a `ps -o pid=,ppid=,...` table.
fn parents(table: &str) -> HashMap<u32, u32> {
    table
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let parent = fields.next()?.parse().ok()?;
            Some((pid, parent))
        })
        .collect()
}

/// `roots` and every process whose chain of parents reaches one of them.
fn below(roots: &HashSet<u32>, parents: &HashMap<u32, u32>) -> HashSet<u32> {
    let mut found = roots.clone();
    for &pid in parents.keys() {
        // A chain is as long as the table at most: a loop in a table read
        // while processes come and go ends there.
        let mut chain = vec![pid];
        let mut current = pid;
        let reaches = loop {
            if found.contains(&current) {
                break true;
            }
            match parents.get(&current) {
                Some(&parent) if parent != current && chain.len() <= parents.len() => {
                    chain.push(parent);
                    current = parent;
                }
                _ => break false,
            }
        };
        if reaches {
            found.extend(chain);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::Output;

    #[derive(Debug)]
    struct Table(Option<&'static str>);

    impl CommandRunner for Table {
        fn run(&self, invocation: &Invocation) -> Result<Output, RunError> {
            assert_eq!(invocation.program.to_string_lossy(), "ps");
            self.0
                .map(Output::success)
                .ok_or_else(|| RunError::NotFound {
                    program: "ps".into(),
                })
        }
    }

    const PS: &str = "\
    1     0 /sbin/launchd
  500     1 /usr/local/bin/volee host --dir /state/hosts run --id h1 -- npx some-agent
  510   500 npm exec some-agent
  520   510 node /cache/some-agent/cli.js
  530   520 /cache/some-agent/helper --stdio
  600     1 /usr/local/bin/volee host --dir /state/hosts run --id h2 -- claude
  610   600 claude
  700     1 -fish
  710   700 claude
";

    fn set(pids: &[u32]) -> HashSet<u32> {
        pids.iter().copied().collect()
    }

    #[test]
    fn a_program_started_through_wrappers_is_below_the_process_volee_started() {
        let found = with_descendants(&Table(Some(PS)), &[510]).unwrap();
        assert_eq!(found, set(&[510, 520, 530]));
    }

    #[test]
    fn processes_elsewhere_are_left_out_and_several_roots_keep_theirs() {
        let found = with_descendants(&Table(Some(PS)), &[510, 610]).unwrap();
        assert_eq!(found, set(&[510, 520, 530, 610]));
        assert!(
            !found.contains(&710),
            "a claude started in a shell is its own"
        );
    }

    #[test]
    fn a_root_that_left_the_table_is_still_itself() {
        let found = with_descendants(&Table(Some(PS)), &[999]).unwrap();
        assert_eq!(found, set(&[999]));
    }

    #[test]
    fn without_ps_the_roots_are_all_that_is_known() {
        let found = with_descendants(&Table(None), &[510]).unwrap();
        assert_eq!(found, set(&[510]));
    }

    #[test]
    fn nothing_is_asked_for_no_root() {
        // A runner that would fail the test if it ran.
        #[derive(Debug)]
        struct Never;
        impl CommandRunner for Never {
            fn run(&self, _: &Invocation) -> Result<Output, RunError> {
                panic!("ps must not run for no root");
            }
        }
        assert!(with_descendants(&Never, &[]).unwrap().is_empty());
    }

    #[test]
    fn a_table_with_a_loop_ends() {
        let table = "  10    11 a\n  11    10 b\n  12    11 c\n";
        let found = below(&set(&[99]), &parents(table));
        assert_eq!(found, set(&[99]));
    }
}
