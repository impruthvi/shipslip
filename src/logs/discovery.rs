//! Filename discovery and channel overrides, independent of log parsing.

use std::collections::{BTreeMap, HashSet};

use super::{is_date, ChannelKind, ChannelSource, ProbeFile, MAX_CHANNELS, MAX_FILES};
use crate::DeployTarget;

pub(super) struct DiscoveredChannel<'a> {
    pub key: String,
    pub name: String,
    pub source: String,
    pub kind: ChannelKind,
    pub files: Vec<&'a ProbeFile>,
    pub omitted_files: usize,
    pub omitted_window_files: usize,
    pub size: u64,
    pub last_write: Option<String>,
}

/// A date suffix is meaningful only at the end of a .log filename. Ordinary
/// hyphens, including supervisor names, remain part of the channel name.
pub(super) fn filename(path: &str) -> Option<(String, Option<&str>)> {
    let stem = path.rsplit('/').next()?.strip_suffix(".log")?;
    if stem.is_empty() {
        return None;
    }
    if let Some(at) = stem.len().checked_sub(11) {
        if stem.as_bytes()[at] == b'-' {
            if let (Some(name), Some(date)) = (stem.get(..at), stem.get(at + 1..)) {
                if !name.is_empty() && is_date(date) {
                    return Some((name.into(), Some(date)));
                }
            }
        }
    }
    Some((stem.into(), None))
}

#[cfg(test)]
pub(super) fn discover<'a>(
    target: &DeployTarget,
    files: &'a [ProbeFile],
    start_date: &str,
) -> (Vec<DiscoveredChannel<'a>>, Vec<String>) {
    discover_inner(target, files, start_date, false, &[])
}

pub(super) fn discover_for_anchor<'a>(
    target: &DeployTarget,
    files: &'a [ProbeFile],
    start_date: &str,
    recorded: &[String],
) -> (Vec<DiscoveredChannel<'a>>, Vec<String>) {
    discover_inner(target, files, start_date, true, recorded)
}

fn discover_inner<'a>(
    target: &DeployTarget,
    files: &'a [ProbeFile],
    start_date: &str,
    baseline: bool,
    recorded: &[String],
) -> (Vec<DiscoveredChannel<'a>>, Vec<String>) {
    let legacy = ChannelSource::from_target(target);
    let mut channels: BTreeMap<String, DiscoveredChannel<'a>> = BTreeMap::new();
    let mut seen = HashSet::new();
    // Probe order gives pinned paths priority, then the legacy path, then auto
    // discovery. A file reached by several aliases is read only once.
    for file in files {
        let inferred = filename(&file.path);
        let pin = target
            .logs
            .iter()
            .find(|(_, settings)| settings.path.as_deref() == Some(file.path.as_str()));
        let is_legacy = legacy.matches(file);
        let (key, date) = if let Some((name, _)) = pin {
            (name.clone(), None)
        } else if is_legacy {
            (
                legacy.name.clone(),
                if legacy.daily {
                    inferred.as_ref().and_then(|(_, date)| *date)
                } else {
                    None
                },
            )
        } else if let Some((name, date)) = inferred {
            (name, date)
        } else {
            continue;
        };
        let settings = target.logs.get(&key);
        if settings.is_some_and(|settings| {
            settings.hide
                || settings
                    .path
                    .as_deref()
                    .is_some_and(|path| path != file.path)
        }) {
            continue;
        }
        if !baseline && date.is_some_and(|date| date < start_date) {
            continue;
        }
        if !seen.insert(file.resolved.as_str()) {
            continue;
        }
        let kind = if date.is_some() {
            ChannelKind::Daily
        } else {
            ChannelKind::Single
        };
        let channel = channels
            .entry(key.clone())
            .or_insert_with(|| DiscoveredChannel {
                name: settings
                    .and_then(|s| s.rename.clone())
                    .unwrap_or_else(|| key.clone()),
                key,
                source: file.path.clone(),
                kind,
                files: Vec::new(),
                omitted_files: 0,
                omitted_window_files: 0,
                size: 0,
                last_write: None,
            });
        if channel.kind != kind {
            channel.kind = ChannelKind::Mixed;
        }
        channel.files.push(file);
        channel.size = channel.size.saturating_add(file.size);
    }
    // A missing explicitly pinned source still gets a row so it cannot vanish
    // silently. Rename/hide alone do not invent nonexistent channels.
    let mut missing: Vec<(String, String, ChannelKind)> = target
        .logs
        .iter()
        .filter_map(|(name, settings)| {
            settings
                .path
                .as_ref()
                .map(|path| (name.clone(), path.clone(), ChannelKind::Single))
        })
        .collect();
    if target.log.is_some() || target.log_daily || channels.is_empty() {
        missing.push((
            legacy.name.clone(),
            legacy.display(),
            if legacy.daily {
                ChannelKind::Daily
            } else {
                ChannelKind::Single
            },
        ));
    }
    for (key, source, kind) in missing {
        let settings = target.logs.get(&key);
        if settings.is_some_and(|s| s.hide) {
            continue;
        }
        channels
            .entry(key.clone())
            .or_insert_with(|| DiscoveredChannel {
                name: settings
                    .and_then(|s| s.rename.clone())
                    .unwrap_or_else(|| key.clone()),
                key,
                source,
                kind,
                files: Vec::new(),
                omitted_files: 0,
                omitted_window_files: 0,
                size: 0,
                last_write: None,
            });
    }
    let mut channels: Vec<_> = channels.into_values().collect();
    channels.sort_by_key(|channel| {
        (
            if target
                .logs
                .get(&channel.key)
                .is_some_and(|s| s.path.is_some())
            {
                0
            } else if channel.key == legacy.name {
                1
            } else {
                2
            },
            channel.key.clone(),
        )
    });
    let mut warnings = Vec::new();
    if channels.len() > MAX_CHANNELS {
        warnings.push(format!(
            "+{} channels not shown (limit {MAX_CHANNELS}; hide unwanted channels in config)",
            channels.len() - MAX_CHANNELS
        ));
        channels.truncate(MAX_CHANNELS);
    }
    for channel in &mut channels {
        channel.last_write = channel
            .files
            .iter()
            .max_by_key(|file| file.mtime)
            .map(|file| file.mtime_local.clone());
        channel.files.sort_by(|a, b| {
            let day = |file: &ProbeFile| {
                filename(&file.path)
                    .and_then(|(_, date)| date)
                    .unwrap_or(file.mtime_local.get(..10).unwrap_or(""))
                    .to_owned()
            };
            day(b)
                .cmp(&day(a))
                .then_with(|| b.mtime.cmp(&a.mtime))
                .then_with(|| b.path.cmp(&a.path))
        });
    }
    if baseline {
        for channel in &mut channels {
            let mut earlier = 0;
            channel.files.retain(|file| {
                if filename(&file.path)
                    .and_then(|(_, date)| date)
                    .is_some_and(|date| date < start_date)
                {
                    earlier += 1;
                    earlier <= 7
                } else {
                    true
                }
            });
        }
    }
    for channel in &mut channels {
        channel.size = channel
            .files
            .iter()
            .fold(0u64, |sum, file| sum.saturating_add(file.size));
    }
    // Share the file cap as well as the byte cap: each channel gets its newest
    // file before any busy daily channel takes its second.
    let mut counts = vec![0; channels.len()];
    let mut remaining = MAX_FILES;
    while remaining > 0 {
        let mut added = false;
        for (channel, count) in channels.iter().zip(&mut counts) {
            if *count < channel.files.len() && remaining > 0 {
                *count += 1;
                remaining -= 1;
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    let mut omitted = 0;
    for (channel, count) in channels.iter_mut().zip(counts) {
        channel.omitted_files = channel.files.len() - count;
        channel.omitted_window_files = channel.files[count..]
            .iter()
            .filter(|file| {
                recorded.contains(&file.path)
                    || filename(&file.path)
                        .and_then(|(_, date)| date)
                        .is_none_or(|date| date >= start_date)
            })
            .count();
        omitted += channel.omitted_files;
        channel.files.truncate(count);
    }
    if omitted > 0 {
        warnings.push(format!(
            "+{omitted} files not read (limit {MAX_FILES}; coverage is incomplete)"
        ));
    }
    (channels, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LogChannel;

    fn target() -> DeployTarget {
        DeployTarget {
            env: "prod".into(),
            production: true,
            ssh_alias: "server".into(),
            path: "/srv/app".into(),
            branch: "main".into(),
            steps: Vec::new(),
            maintenance: false,
            watch_log: false,
            log: None,
            log_daily: false,
            smoke_url: None,
            timezone: None,
            logs: BTreeMap::new(),
        }
    }

    fn file(path: &str) -> ProbeFile {
        ProbeFile {
            path: path.into(),
            resolved: path.into(),
            inode: 7,
            size: 100,
            mtime: 1000,
            mtime_local: "2026-10-01 01:00:00".into(),
            inside: true,
            checksum: "checksum".into(),
            anchor_check: None,
        }
    }

    #[test]
    fn filenames_only_split_a_final_date_suffix() {
        assert_eq!(
            filename("storage/logs/horizon-supervisor-1.log"),
            Some(("horizon-supervisor-1".into(), None))
        );
        assert_eq!(
            filename("storage/logs/pay-2026-10-01.log"),
            Some(("pay".into(), Some("2026-10-01")))
        );
        assert_eq!(
            filename("storage/logs/pay ments.log"),
            Some(("pay ments".into(), None))
        );
        assert_eq!(
            filename("storage/logs/éééééé.log"),
            Some(("éééééé".into(), None))
        );
        assert_eq!(filename("storage/logs/laravel-2026-10-01.log.1"), None);
        assert_eq!(filename("storage/logs/.log"), None);
    }

    #[test]
    fn discovery_merges_daily_and_single_and_applies_overrides_before_reading() {
        let mut target = target();
        target.logs.insert(
            "laravel".into(),
            LogChannel {
                rename: Some("app".into()),
                ..LogChannel::default()
            },
        );
        target.logs.insert(
            "worker".into(),
            LogChannel {
                hide: true,
                ..LogChannel::default()
            },
        );
        target.logs.insert(
            "payments".into(),
            LogChannel {
                path: Some("/srv/private/billing.txt".into()),
                ..LogChannel::default()
            },
        );
        let files = [
            file("/srv/private/billing.txt"),
            file("storage/logs/laravel.log"),
            file("storage/logs/laravel-2026-10-01.log"),
            file("storage/logs/laravel-2026-09-30.log"),
            file("storage/logs/worker.log"),
            file("storage/logs/payments.log"),
        ];
        let (channels, warnings) = discover(&target, &files, "2026-10-01");
        assert!(warnings.is_empty());
        assert_eq!(channels.len(), 2);
        assert_eq!(channels[0].key, "payments");
        assert_eq!(channels[0].files[0].path, "/srv/private/billing.txt");
        assert_eq!(channels[1].name, "app");
        assert_eq!(channels[1].kind, ChannelKind::Mixed);
        assert_eq!(channels[1].files.len(), 2);
    }

    #[test]
    fn aliases_are_not_counted_twice_and_missing_pins_remain_visible() {
        let mut target = target();
        target.log = Some("/srv/app/storage/logs/custom.log".into());
        target.logs.insert(
            "missing".into(),
            LogChannel {
                path: Some("/srv/missing.log".into()),
                ..LogChannel::default()
            },
        );
        let explicit = file("/srv/app/storage/logs/custom.log");
        let mut alias = file("storage/logs/custom.log");
        alias.resolved = explicit.resolved.clone();
        let files = [explicit, alias];
        let (channels, _) = discover(&target, &files, "2026-10-01");
        assert_eq!(channels.iter().map(|c| c.files.len()).sum::<usize>(), 1);
        assert!(channels
            .iter()
            .any(|c| c.key == "missing" && c.files.is_empty()));
    }

    #[test]
    fn daily_dates_take_priority_over_a_recent_touch_of_an_old_file() {
        let mut old = file("storage/logs/laravel-2026-10-01.log");
        old.mtime = 2000;
        let recent = file("storage/logs/laravel-2026-10-02.log");
        let files = [old, recent];
        let (channels, _) = discover(&target(), &files, "2026-10-01");
        assert!(channels[0].files[0].path.ends_with("10-02.log"));
        assert_eq!(channels[0].size, 200);
    }

    #[test]
    fn limits_report_omissions_and_share_file_slots_across_channels() {
        let files: Vec<_> = (0..25)
            .map(|i| file(&format!("storage/logs/c{i:02}.log")))
            .collect();
        let (channels, warnings) = discover(&target(), &files, "2026-10-01");
        assert_eq!(channels.len(), MAX_CHANNELS);
        assert!(warnings[0].contains("+5 channels not shown"));
        let files: Vec<_> = (0..3)
            .flat_map(|channel| {
                (1..=31)
                    .map(move |day| file(&format!("storage/logs/c{channel}-2026-10-{day:02}.log")))
            })
            .collect();
        let (channels, warnings) = discover(&target(), &files, "2026-10-01");
        assert_eq!(
            channels.iter().map(|c| c.files.len()).sum::<usize>(),
            MAX_FILES
        );
        assert!(channels.iter().all(|c| c.files.len() >= 16));
        assert_eq!(channels.iter().map(|c| c.omitted_files).sum::<usize>(), 43);
        assert!(warnings[0].contains("+43 files not read"));
    }
}
