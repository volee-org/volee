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
