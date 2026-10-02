//! Separate bounded reads for the window and the bytes immediately before it.
use super::{anchor, discovery, ProbeFile, ReadPlan};
use crate::marker::Marker;
use std::collections::HashMap;

pub(super) struct Plan<'a> {
    pub window: Vec<ReadPlan<'a>>,
    pub baseline: Vec<ReadPlan<'a>>,
    pub cuts: HashMap<String, u64>,
    pub expected_window: usize,
}

pub(super) fn cuts(
    files: &[&ProbeFile],
    date: &str,
    marker: Option<&Marker>,
    probe: &anchor::Probe,
    warnings: &mut Vec<String>,
) -> HashMap<String, u64> {
    let mut cuts = HashMap::new();
    for file in files {
        let recorded = marker.and_then(|marker| {
            marker
                .files
                .iter()
                .find(|recorded| recorded.path == file.path)
        });
        if let Some(recorded) = recorded {
            if recorded.inode == file.inode
                && file.size >= recorded.size
                && probe
                    .splits
                    .get(&file.path)
                    .is_some_and(|(size, sum)| *size == recorded.size && sum == &recorded.checksum)
            {
                cuts.insert(file.path.clone(), recorded.size);
                continue;
            }
            warnings.push(format!("{}: recorded offset no longer matches (rotation/truncation/checksum); splitting by timestamp", file.path));
            // Even an old-named daily file may have regrown after the run.
            continue;
        }
        if let Some(day) = discovery::filename(&file.path).and_then(|(_, date)| date) {
            if day < date {
                cuts.insert(file.path.clone(), file.size);
            } else if day > date {
                cuts.insert(file.path.clone(), 0);
            }
        }
    }
    cuts
}

pub(super) fn needs(files: &[&ProbeFile], cuts: &HashMap<String, u64>) -> (u64, u64) {
    let mut window = 0u64;
    let mut baseline = 0u64;
    for file in files {
        let cut = cuts.get(&file.path).copied();
        window = window.saturating_add(file.size - cut.unwrap_or(0));
        baseline = baseline.saturating_add(cut.unwrap_or(file.size));
    }
    (window, baseline)
}

pub(super) fn plan<'a>(
    files: &[&'a ProbeFile],
    cuts: HashMap<String, u64>,
    window_budget: u64,
    baseline_budget: u64,
    date: &str,
) -> Plan<'a> {
    let mut window = Vec::new();
    let mut left = window_budget;
    let mut expected_window = 0;
    for file in files {
        let cut = cuts.get(&file.path).copied().unwrap_or(0);
        if cut == file.size && file.size > 0 {
            continue;
        }
        expected_window += 1;
        if left == 0 && file.size > 0 {
            continue;
        }
        let count = (file.size - cut).min(left);
        left -= count;
        window.push(ReadPlan {
            file,
            start: file.size - count,
            count,
        });
    }
    let mut baseline = Vec::new();
    left = baseline_budget;
    let mut earlier_files = 0;
    for file in files {
        if left == 0 {
            break;
        }
        let end = cuts.get(&file.path).copied().unwrap_or_else(|| {
            window
                .iter()
                .find(|read| read.file.path == file.path)
                .map(|read| read.start)
                .unwrap_or(file.size)
        });
        if end == file.size
            && discovery::filename(&file.path)
                .and_then(|(_, day)| day)
                .is_some_and(|day| day < date)
        {
            earlier_files += 1;
            if earlier_files > 7 {
                continue;
            }
        }
        let count = end.min(left);
        if count == 0 {
            continue;
        }
        left -= count;
        baseline.push(ReadPlan {
            file,
            start: end - count,
            count,
        });
    }
    Plan {
        window,
        baseline,
        cuts,
        expected_window,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file(path: &str, size: u64) -> ProbeFile {
        ProbeFile {
            path: path.into(),
            resolved: path.into(),
            inode: 7,
            size,
            mtime: 10,
            mtime_local: "2026-10-01 00:00:00".into(),
            inside: true,
            checksum: "sum".into(),
            anchor_check: None,
        }
    }
    #[test]
    fn known_offsets_separate_the_window_and_comparison_without_overlap() {
        let file = file("laravel.log", 1000);
        let cuts = HashMap::from([(file.path.clone(), 600)]);
        assert_eq!(needs(&[&file], &cuts), (400, 600));
        let plan = plan(&[&file], cuts, 400, 200, "2026-10-01");
        assert_eq!((plan.window[0].start, plan.window[0].count), (600, 400));
        assert_eq!((plan.baseline[0].start, plan.baseline[0].count), (400, 200));
    }
    #[test]
    fn a_daily_baseline_checks_at_most_seven_earlier_files() {
        let files: Vec<_> = (1..=10)
            .rev()
            .map(|day| file(&format!("laravel-2026-09-{day:02}.log"), 100))
            .collect();
        let refs: Vec<_> = files.iter().collect();
        let cuts = files
            .iter()
            .map(|file| (file.path.clone(), file.size))
            .collect();
        let plan = plan(&refs, cuts, 10000, 10000, "2026-10-01");
        assert!(plan.window.is_empty());
        assert_eq!(plan.baseline.len(), 7);
        assert_eq!(
            plan.baseline.iter().map(|read| read.count).sum::<u64>(),
            700
        );
    }
}
