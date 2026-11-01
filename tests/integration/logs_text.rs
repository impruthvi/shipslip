//! `slip logs` text for snapshots read from the real sshd, through the CLI's
//! own renderer.

use shipslip::logs::{Group, Groups, Level, Snapshot};
use shipslip::records;

#[path = "../../src/bin/slip/logs_words.rs"]
#[allow(dead_code)]
mod logs_words;

fn record(snapshot: &Snapshot, level: Level, grep: Option<&str>) -> records::LogsSummary {
    records::logs_summary(snapshot, &Groups::default(), level, grep)
}

pub fn header(snapshot: &Snapshot) -> String {
    logs_words::header(&record(snapshot, Level::Debug, None))
}

pub fn channels(snapshot: &Snapshot) -> String {
    logs_words::channels(&record(snapshot, Level::Debug, None))
}

pub fn raw(snapshot: &Snapshot, level: Level, grep: Option<&str>) -> String {
    logs_words::raw_entries(&record(snapshot, level, grep), &snapshot.entries)
}

pub fn summary(snapshot: &Snapshot, groups: &Groups, level: Level, grep: Option<&str>) -> String {
    logs_words::summary(&records::logs_summary(snapshot, groups, level, grep), false)
}

pub fn detail(snapshot: &Snapshot, group: &Group, full: bool) -> String {
    logs_words::detail(
        &records::logs_detail(snapshot, group, Level::Debug, None),
        full,
    )
}

pub fn miss(snapshot: &Snapshot, query: &str) -> String {
    logs_words::miss(&record(snapshot, Level::Error, None), query)
}

pub fn window(snapshot: &Snapshot) -> String {
    logs_words::window_label(&snapshot.anchor)
}
