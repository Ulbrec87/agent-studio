//! Process tree: which processes live below an agent's PTY child (Hydra `processTree.ts`, which
//! parsed `ps`; here the table comes from `sysinfo`, so it also works on Windows).

use std::collections::{HashMap, HashSet};

/// Parent pid by pid.
pub type ProcessTable = HashMap<u32, u32>;

/// Pids below `root_pid` in the process tree (the root itself excluded). Cycle-safe.
pub fn descendant_pids(root_pid: u32, table: &ProcessTable) -> HashSet<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&pid, &ppid) in table {
        if pid != ppid {
            children.entry(ppid).or_default().push(pid);
        }
    }
    let mut found = HashSet::new();
    let mut queue = vec![root_pid];
    while let Some(current) = queue.pop() {
        for &child in children
            .get(&current)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            if child == root_pid || !found.insert(child) {
                continue;
            }
            queue.push(child);
        }
    }
    found
}

/// Reads the whole process table from the OS.
pub fn read_system_process_table() -> ProcessTable {
    use sysinfo::{ProcessesToUpdate, System};
    let Ok(mut system) = System::new() else {
        return ProcessTable::new();
    };
    system.refresh_processes(ProcessesToUpdate::All, true);
    system
        .processes()
        .iter()
        .filter_map(|(pid, p)| p.parent().map(|parent| (pid.as_u32(), parent.as_u32())))
        .collect()
}

/// Reads the process table at most once per `min_interval_ms`; between runs the last table is
/// returned. Empty when the source fails.
pub struct ProcessTableReader {
    table: ProcessTable,
    last_run_at: Option<i64>,
    source: Box<dyn FnMut() -> ProcessTable + Send>,
    min_interval_ms: i64,
}

impl Default for ProcessTableReader {
    fn default() -> Self {
        Self::new(Box::new(read_system_process_table), 10_000)
    }
}

impl ProcessTableReader {
    pub fn new(source: Box<dyn FnMut() -> ProcessTable + Send>, min_interval_ms: i64) -> Self {
        Self {
            table: HashMap::new(),
            last_run_at: None,
            source,
            min_interval_ms,
        }
    }

    pub fn read(&mut self, now: i64) -> &ProcessTable {
        if self
            .last_run_at
            .is_none_or(|at| now - at >= self.min_interval_ms)
        {
            self.last_run_at = Some(now);
            self.table = (self.source)();
        }
        &self.table
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn table(pairs: &[(u32, u32)]) -> ProcessTable {
        pairs.iter().copied().collect()
    }

    #[test]
    fn finds_children_and_grandchildren() {
        let t = table(&[(10, 1), (11, 10), (12, 11), (13, 10), (20, 1)]);
        assert_eq!(descendant_pids(10, &t), HashSet::from([11, 12, 13]));
        assert!(descendant_pids(20, &t).is_empty());
        assert!(descendant_pids(999, &t).is_empty());
    }

    #[test]
    fn survives_cycles_and_self_parents() {
        let t = table(&[(1, 1), (2, 3), (3, 2), (4, 2)]);
        assert_eq!(descendant_pids(2, &t), HashSet::from([3, 4]));
        assert!(descendant_pids(1, &t).is_empty());
    }

    #[test]
    fn the_reader_throttles_the_source() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let mut reader = ProcessTableReader::new(
            Box::new(move || {
                c.fetch_add(1, Ordering::SeqCst);
                table(&[(2, 1)])
            }),
            10_000,
        );
        assert_eq!(reader.read(0).len(), 1);
        assert_eq!(reader.read(9_999).len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        reader.read(10_000);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn the_system_table_knows_this_process_and_its_parent() {
        let t = read_system_process_table();
        assert!(t.contains_key(&std::process::id()));
    }

    #[cfg(unix)]
    #[test]
    fn a_shell_wrapping_a_child_exposes_the_grandchild() {
        // `sh -c 'sleep 5 & wait'`: the sleep is a descendant of the shell.
        let mut child = std::process::Command::new("sh")
            .args(["-c", "sleep 5 & wait"])
            .spawn()
            .unwrap();
        let mut found = false;
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if !descendant_pids(child.id(), &read_system_process_table()).is_empty() {
                found = true;
                break;
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        assert!(found, "the shell's child was not found in the process tree");
    }
}
