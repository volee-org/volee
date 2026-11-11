//! The process table, read once with `ps`: which process started which, to
//! walk from a session's process up to the pane that runs it.

use std::collections::HashMap;

/// The longest chain of parents followed: deeper trees are not shells and
/// agents, and a loop must end.
const MAX_DEPTH: usize = 64;

/// Every process's parent, from the output of `ps -axo pid=,ppid=`. Lines
/// that are not two numbers are skipped.
pub(crate) fn parents(output: &str) -> HashMap<u32, u32> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let parent = fields.next()?.parse().ok()?;
            Some((pid, parent))
        })
        .collect()
}

/// For every process that holds a terminal, the process group in the
/// foreground of that terminal, from the output of `ps -axo pid=,tpgid=`.
/// The leader of that group is the program a person sees in the terminal. A
/// process without a terminal has none (`0` or `-1`) and is left out.
pub(crate) fn foregrounds(output: &str) -> HashMap<u32, u32> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let foreground: u32 = fields.next()?.parse().ok()?;
            (foreground > 0).then_some((pid, foreground))
        })
        .collect()
}

/// `pid` and its ancestors, nearest first, up to the first process the
/// system started (`pid` 1) excluded.
pub(crate) fn ancestors(pid: u32, parents: &HashMap<u32, u32>) -> Vec<u32> {
    let mut chain = vec![pid];
    let mut current = pid;
    while let Some(&parent) = parents.get(&current) {
        if parent <= 1 || chain.contains(&parent) || chain.len() >= MAX_DEPTH {
            break;
        }
        chain.push(parent);
        current = parent;
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_reads_ps_output() {
        let table = parents("  101     1\n  250   101\nnoise\n  251   250 extra\n\n");
        assert_eq!(table.get(&250), Some(&101));
        assert_eq!(table.get(&251), Some(&250));
        assert_eq!(table.len(), 3);
    }

    #[test]
    fn the_foreground_of_a_terminal_is_read_for_the_processes_that_hold_one() {
        // A program started in a pane, a shell with a foreground job, that
        // job, and two processes without a terminal.
        let table =
            foregrounds(" 500   500\n 600   640\n 640   640\n 700     0\n 701    -1\nnoise\n");
        assert_eq!(table.get(&500), Some(&500));
        assert_eq!(table.get(&600), Some(&640));
        assert_eq!(table.get(&700), None);
        assert_eq!(table.get(&701), None);
        assert_eq!(table.len(), 3);
    }

    #[test]
    fn ancestors_stop_at_the_first_process_and_at_loops() {
        let table = parents("101 1\n250 101\n251 250\n");
        assert_eq!(ancestors(251, &table), [251, 250, 101]);
        // A process the table does not know is its own chain.
        assert_eq!(ancestors(999, &table), [999]);
        // A table that loops (pids reused while ps ran) still ends.
        let looping = parents("5 6\n6 5\n");
        assert_eq!(ancestors(5, &looping), [5, 6]);
    }
}
