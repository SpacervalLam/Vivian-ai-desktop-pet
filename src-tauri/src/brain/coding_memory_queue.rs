//! FIFO within each project, bounded concurrency across independent projects.
use std::{collections::HashSet, path::PathBuf};

pub fn ready(
    jobs: &[(PathBuf, i64, bool)],
    active: &HashSet<PathBuf>,
    now: i64,
    capacity: usize,
) -> Vec<usize> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for (index, (project, next_at, failed)) in jobs.iter().enumerate() {
        if *failed || !seen.insert(project) {
            continue;
        }
        if !active.contains(project) && *next_at <= now && result.len() < capacity {
            result.push(index);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delayed_or_busy_project_does_not_block_other_projects() {
        let a = PathBuf::from("a");
        let b = PathBuf::from("b");
        let c = PathBuf::from("c");
        let jobs = vec![
            (a.clone(), 100, false),
            (a.clone(), 0, false),
            (b.clone(), 0, false),
            (c.clone(), 0, false),
        ];
        assert_eq!(ready(&jobs, &HashSet::new(), 0, 2), vec![2, 3]);
        assert_eq!(ready(&jobs, &HashSet::from([b]), 100, 2), vec![0, 3]);
        assert_eq!(ready(&jobs, &HashSet::new(), 100, 1), vec![0]);
    }
}
