//! `slip logs`: a read-only snapshot of an environment's log channels.
//!
//! Two remote calls, neither of which writes anything on the server:
//!
//! 1. The probe checks the timezone, converts the window start into the log's
//!    local time, and stats each candidate file.
//! 2. The read re-checks every file it was told to read (path, inode, size),
//!    then sends the newest bytes within budget, gzip-compressed.
//!
//! Laravel writes times without an offset. Clients validate explicit windows
//! locally before connecting; the probe converts them again against the server
//! clock with `TZ=<zone> date`. Entries are compared as local
//! `YYYY-MM-DD HH:MM:SS` strings. Within a file, position decides: once an
//! entry at or after the window start is seen, every later entry counts, which
//! keeps the repeated hour of a DST fall-back.

use std::collections::{HashMap, HashSet};
use std::io::Read as _;

use base64::Engine as _;
use sha2::{Digest, Sha256};

use crate::observation::{
    clip_utf8, parse_header, signature, EntrySignature, GroupKey, ENTRY_BYTES, MAX_GROUPS,
    MAX_VARIANTS,
};
use crate::runner::run_collect;
use crate::script::shell_quote;
use crate::transport::{Transport, TransportError};
use crate::DeployTarget;

mod anchor;
mod baseline;
mod discovery;

pub use anchor::{
    Anchor, AnchorFallback, CheckoutFallbackReason, MarkerFallbackReason, RunCheck,
    RunFallbackReason,
};

const DEFAULT_WINDOW: u64 = 24 * 3600;
const MAX_WINDOW: u64 = 30 * 24 * 3600;
const CHANNEL_BYTES: u64 = 4 * 1024 * 1024;
const TOTAL_BYTES: u64 = 12 * 1024 * 1024;
const FLOOR_BYTES: u64 = 256 * 1024;
const BASELINE_CHANNEL_BYTES: u64 = 2 * 1024 * 1024;
const BASELINE_TOTAL_BYTES: u64 = 6 * 1024 * 1024;
const BASELINE_FLOOR_BYTES: u64 = 128 * 1024;
const MESSAGE_BYTES: usize = 512;
const ID_LEN: usize = 5;
/// Letters only, so an ID is never mistaken for a row number; no `l` or `o`.
const ID_ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyz";
const CLOCK_SKEW_SECS: i64 = 15 * 60;
const MAX_FILES: usize = 50;
const MAX_CHANNELS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Debug,
    Info,
    Notice,
    Warning,
    Error,
    Critical,
    Alert,
    Emergency,
}

impl Level {
    const ALL: [Level; 8] = [
        Level::Debug,
        Level::Info,
        Level::Notice,
        Level::Warning,
        Level::Error,
        Level::Critical,
        Level::Alert,
        Level::Emergency,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Notice => "notice",
            Level::Warning => "warning",
            Level::Error => "error",
            Level::Critical => "critical",
            Level::Alert => "alert",
            Level::Emergency => "emergency",
        }
    }

    /// Parses a level name in any case.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|level| level.name().eq_ignore_ascii_case(name))
    }

    pub fn names() -> String {
        Self::ALL.map(Level::name).join(", ")
    }
}

/// The start of the window to show.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Since {
    /// Seconds before the server's current time.
    Ago(u64),
    /// `YYYY-MM-DD` or `YYYY-MM-DD HH:MM`, in the log's timezone.
    At(String),
}

impl Since {
    /// Checks the window against this Mac's clock before opening SSH.
    /// The server probe still checks it against the server's clock.
    pub fn validate(&self, zone: &str) -> Result<(), LogsError> {
        let seconds = match self {
            Self::Ago(seconds) => *seconds,
            Self::At(at) => {
                if !matches!(Self::parse(at), Ok(Self::At(_))) {
                    return Err(LogsError::BadSince(at.clone()));
                }
                let time = if at.len() == 10 {
                    format!("{at} 00:00:00")
                } else {
                    format!("{at}:00")
                };
                let mut command = std::process::Command::new("date");
                command.env("TZ", zone);
                #[cfg(target_os = "macos")]
                command.args(["-j", "-f", "%Y-%m-%d %H:%M:%S", &time, "+%s"]);
                #[cfg(not(target_os = "macos"))]
                command.args(["-d", &time, "+%s"]);
                let output = command.output().map_err(|error| {
                    LogsError::Unavailable(format!("cannot validate --since locally: {error}"))
                })?;
                let start = std::str::from_utf8(&output.stdout)
                    .ok()
                    .filter(|_| output.status.success())
                    .and_then(|text| text.trim().parse::<i64>().ok())
                    .ok_or_else(|| LogsError::BadSince(at.clone()))?;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                i64::try_from(now)
                    .unwrap_or(i64::MAX)
                    .saturating_sub(start)
                    .max(0) as u64
            }
        };
        if seconds > MAX_WINDOW {
            Err(LogsError::TooOld)
        } else {
            Ok(())
        }
    }

    /// Accepts `30m`, `6h`, `7d`, `YYYY-MM-DD` and `YYYY-MM-DD HH:MM`.
    pub fn parse(value: &str) -> Result<Self, String> {
        let usage = || {
            format!(
                "`--since {value}` must be like 30m, 6h, 7d, 2026-10-01 or \"2026-10-01 14:00\""
            )
        };
        if !value.is_ascii() {
            return Err(usage());
        }
        if let Some(unit) = value
            .chars()
            .last()
            .filter(|c| matches!(c, 'm' | 'h' | 'd'))
        {
            let amount: u64 = value[..value.len() - 1].parse().map_err(|_| usage())?;
            let seconds = amount.saturating_mul(match unit {
                'm' => 60,
                'h' => 3600,
                _ => 86_400,
            });
            if seconds == 0 {
                return Err(usage());
            }
            if seconds > MAX_WINDOW {
                return Err("`--since` can reach back at most 30 days".into());
            }
            return Ok(Since::Ago(seconds));
        }
        let valid = match value.len() {
            10 => is_date(value),
            16 => is_date(&value[..10]) && value.as_bytes()[10] == b' ' && is_clock(&value[11..]),
            _ => false,
        };
        if valid {
            Ok(Since::At(value.into()))
        } else {
            Err(usage())
        }
    }
}

fn is_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !value
            .char_indices()
            .all(|(index, character)| index == 4 || index == 7 || character.is_ascii_digit())
    {
        return false;
    }
    let year = value[..4].parse::<u32>().unwrap_or(0);
    let month = value[5..7].parse::<u32>().unwrap_or(0);
    let day = value[8..10].parse::<u32>().unwrap_or(0);
    let days = match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => return false,
    };
    year != 0 && (1..=days).contains(&day)
}

fn is_clock(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() == 5
        && b[2] == b':'
        && value
            .char_indices()
            .all(|(i, c)| i == 2 || c.is_ascii_digit())
        && value[..2].parse::<u32>().is_ok_and(|h| h < 24)
        && value[3..].parse::<u32>().is_ok_and(|m| m < 60)
}

#[derive(Debug, Clone, Default)]
pub struct Request {
    /// `None` means the last 24 hours.
    pub since: Option<Since>,
    /// Replaces both the per-channel and the total read budget.
    pub max_bytes: Option<u64>,
    /// Read configured paths outside `storage/logs`; only for trusted config.
    pub allow_outside: bool,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum LogsError {
    #[error("{0}")]
    Transport(#[from] TransportError),
    #[error("server has no timezone data for `{0}`")]
    ZoneMissing(String),
    #[error("the server could not read `--since {0}` as a time")]
    BadSince(String),
    #[error("`--since` can reach back at most 30 days")]
    TooOld,
    #[error(
        "log path `{path}` is outside storage/logs; run `slip trust {env}` to allow reading it"
    )]
    Outside { path: String, env: String },
    #[error("could not read logs: {0}")]
    Unavailable(String),
}

/// One log stream: a single file, or a series of daily files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub name: String,
    pub key: String,
    pub kind: ChannelKind,
    /// What was configured or looked for, for messages when nothing is found.
    pub source: String,
    pub files: Vec<String>,
    /// Bytes requested for this channel by the budget.
    pub budget: u64,
    pub bytes_read: u64,
    /// Every entry since the window start was read.
    pub complete: bool,
    /// The oldest entry read, when `complete` is false.
    pub covered_from: Option<String>,
    /// Files that changed between the probe and the read and were skipped.
    pub changed: Vec<String>,
    pub size: u64,
    pub last_write: Option<String>,
    pub format: Format,
    pub omitted_files: usize,
    pub baseline: Baseline,
}

/// The bounded comparison immediately before this snapshot's window.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Baseline {
    pub budget: u64,
    pub bytes_read: u64,
    pub available: bool,
    pub complete: bool,
    pub covered_from: Option<String>,
    pub covered_to: Option<String>,
    pub files: Vec<String>,
    pub format: Option<Format>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    Single,
    Daily,
    Mixed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Format {
    Recognized,
    Partial(u64),
    Unrecognized,
    Empty,
    NotRead,
}

impl Format {
    fn has_unknown_content(&self) -> bool {
        matches!(self, Self::Partial(_) | Self::Unrecognized | Self::NotRead)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawBlock {
    pub channel: usize,
    pub file: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub channel: usize,
    pub file: String,
    /// Byte offset of the entry's first line in its file.
    pub offset: u64,
    /// `YYYY-MM-DD HH:MM:SS` in the log's timezone, when it could be read.
    pub time: Option<String>,
    pub level: Level,
    /// The entry as written, header line first.
    pub text: String,
    pub truncated: bool,
    /// Where the message starts in `text`, after `[time] env.LEVEL: `.
    message_start: usize,
}

impl Entry {
    /// The message and continuation lines, as the deploy watch signs them.
    pub fn message(&self) -> &str {
        &self.text[self.message_start..]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub env: String,
    pub zone: String,
    /// Evidence used to choose the window start.
    pub anchor: Anchor,
    /// Candidates that could not be used, in selection order.
    pub anchor_fallbacks: Vec<AnchorFallback>,
    /// The window start in the log's timezone.
    pub start: String,
    /// When the server was probed, in the log's timezone: the window's end.
    pub read_at: Option<String>,
    pub channels: Vec<Channel>,
    /// Entries inside the window, in file order per file.
    pub entries: Vec<Entry>,
    /// Bounded file samples whose format was not fully recognized.
    pub raw_blocks: Vec<RawBlock>,
    pub baseline_entries: Vec<Entry>,
    pub baseline_enabled: bool,
    /// The zone's UTC offset at the window start differs from now.
    pub clock_changed: bool,
    pub warnings: Vec<String>,
    /// Base64-encoded compressed data bytes received over the connection.
    pub bytes_transferred: u64,
}

/// Discovers and reads `target`'s log channels without changing the server.
pub async fn snapshot<T: Transport>(
    transport: &T,
    target: &DeployTarget,
    request: &Request,
) -> Result<Snapshot, LogsError> {
    let zone = target.timezone.clone().unwrap_or_else(|| "UTC".into());
    let since = request.since.clone().unwrap_or(Since::Ago(DEFAULT_WINDOW));
    let source = ChannelSource::from_target(target);

    let (code, lines) = run_collect(
        transport,
        &probe_script(
            &target.path,
            &zone,
            &since,
            &source,
            request.allow_outside,
            &target.logs,
        ),
    )
    .await;
    let code = code?;
    let mut probe = parse_probe(code, &lines)?;
    let time = match probe.time {
        ProbeTime::ZoneMissing => return Err(LogsError::ZoneMissing(zone)),
        ProbeTime::BadSince => {
            let Since::At(at) = &since else {
                return Err(LogsError::Unavailable("server rejected the window".into()));
            };
            return Err(LogsError::BadSince(at.clone()));
        }
        ProbeTime::Known(time) => time,
    };
    if request.since.is_some() && time.now.saturating_sub(time.start) > MAX_WINDOW {
        return Err(LogsError::TooOld);
    }
    let choice = if request.since.is_none() {
        probe.anchor.choose(time)
    } else {
        anchor::Choice::new(
            Anchor::Since {
                since: since.clone(),
            },
            time,
            None,
            Vec::new(),
        )
    };
    let time = choice.time;
    for file in &mut probe.files {
        if let Some(recorded) = choice.marker.as_ref().and_then(|marker| {
            marker
                .files
                .iter()
                .find(|recorded| recorded.path == file.path)
        }) {
            if recorded.inode == file.inode
                && file.size >= recorded.size
                && probe
                    .anchor
                    .splits
                    .get(&file.path)
                    .is_some_and(|(size, sum)| *size == recorded.size && sum == &recorded.checksum)
            {
                file.anchor_check = Some((recorded.size, recorded.checksum.clone()));
            }
        }
    }
    let recorded: Vec<_> = choice
        .marker
        .as_ref()
        .map(|marker| marker.files.iter().map(|file| file.path.clone()).collect())
        .unwrap_or_default();
    let (sources, mut warnings) =
        discovery::discover_for_anchor(target, &probe.files, &time.start_date, &recorded);
    warnings.extend(choice.warnings);
    for file in sources.iter().flat_map(|source| &source.files) {
        if !file.inside && !request.allow_outside {
            return Err(LogsError::Outside {
                path: file.path.clone(),
                env: target.env.clone(),
            });
        }
    }
    if !probe.files.iter().any(|file| file.mtime >= time.start) {
        warnings.push(format!("No file in storage/logs was written since {}. If LOG_CHANNEL is stderr, syslog, or a service, slip can't see it.", time.start_local));
    }
    let cuts: Vec<_> = sources
        .iter()
        .map(|source| {
            baseline::cuts(
                &source.files,
                &time.start_date,
                choice.marker.as_ref(),
                &probe.anchor,
                &mut warnings,
            )
        })
        .collect();
    let needs: Vec<_> = sources
        .iter()
        .zip(&cuts)
        .map(|(source, cuts)| baseline::needs(&source.files, cuts))
        .collect();
    let budgets = water_fill(
        &needs.iter().map(|need| need.0).collect::<Vec<_>>(),
        request.max_bytes.unwrap_or(TOTAL_BYTES),
        request.max_bytes.unwrap_or(CHANNEL_BYTES),
        FLOOR_BYTES,
    );
    let base_budgets = water_fill(
        &needs.iter().map(|need| need.1).collect::<Vec<_>>(),
        BASELINE_TOTAL_BYTES,
        BASELINE_CHANNEL_BYTES,
        BASELINE_FLOOR_BYTES,
    );
    let plans: Vec<_> = sources
        .iter()
        .zip(cuts)
        .zip(&budgets)
        .zip(&base_budgets)
        .map(|(((source, cuts), window), baseline)| {
            baseline::plan(&source.files, cuts, *window, *baseline, &time.start_date)
        })
        .collect();
    let plan: Vec<_> = plans
        .iter()
        .flat_map(|plan| plan.window.iter().chain(&plan.baseline))
        .cloned()
        .collect();
    let mut snapshot = Snapshot {
        env: target.env.clone(),
        zone,
        anchor: choice.anchor,
        anchor_fallbacks: choice.fallbacks,
        start: time.start_local.clone(),
        read_at: local_time(time.now, &time.offset_now),
        channels: Vec::new(),
        entries: Vec::new(),
        raw_blocks: Vec::new(),
        baseline_entries: Vec::new(),
        baseline_enabled: true,
        clock_changed: time.offset_start != time.offset_now,
        warnings,
        bytes_transferred: 0,
    };
    let reads = if plan.is_empty() {
        Vec::new()
    } else {
        let (code, lines) = run_collect(
            transport,
            &read_script(&target.path, &plan, request.allow_outside),
        )
        .await;
        parse_read(
            code?,
            &lines,
            &plan.iter().map(|read| read.count).collect::<Vec<_>>(),
        )?
    };
    let mut reads = reads.into_iter();
    let mut newest_entry: Option<(String, &ProbeFile)> = None;
    for (index, (((source, budget), base_budget), plan)) in sources
        .iter()
        .zip(budgets)
        .zip(base_budgets)
        .zip(plans)
        .enumerate()
    {
        let mut channel = Channel {
            name: source.name.clone(),
            key: source.key.clone(),
            kind: source.kind,
            source: source.source.clone(),
            files: source.files.iter().map(|file| file.path.clone()).collect(),
            budget,
            bytes_read: 0,
            complete: source.omitted_window_files == 0 && plan.window.len() == plan.expected_window,
            covered_from: None,
            changed: Vec::new(),
            size: source.size,
            last_write: source.last_write.clone(),
            format: Format::NotRead,
            omitted_files: source.omitted_files,
            baseline: Baseline {
                budget: base_budget,
                complete: source.omitted_files == 0,
                ..Baseline::default()
            },
        };
        let mut chunks = Vec::new();
        let mut stats = FormatStats::default();
        let mut window_successful = false;
        let data: Vec<_> = reads
            .by_ref()
            .take(plan.window.len() + plan.baseline.len())
            .collect();
        for (slot, (read, data)) in plan
            .window
            .iter()
            .chain(&plan.baseline)
            .zip(data)
            .enumerate()
        {
            let window = slot < plan.window.len();
            let Some(data) = data else {
                if !channel.changed.contains(&read.file.path) {
                    channel.changed.push(read.file.path.clone());
                }
                if window {
                    channel.complete = false;
                }
                channel.baseline.complete = false;
                continue;
            };
            snapshot.bytes_transferred += data.transferred;
            let mut entries = parse_entries(&data.bytes, read.start, index, &read.file.path);
            let split = if let Some(cut) = plan.cuts.get(&read.file.path) {
                entries
                    .iter()
                    .position(|entry| entry.offset >= *cut)
                    .unwrap_or(entries.len())
            } else {
                entries
                    .iter()
                    .position(|entry| {
                        entry
                            .time
                            .as_deref()
                            .is_some_and(|at| at >= time.start_local.as_str())
                    })
                    .unwrap_or(entries.len())
            };
            let before_end = plan
                .cuts
                .get(&read.file.path)
                .map(|cut| cut.saturating_sub(read.start))
                .unwrap_or_else(|| {
                    entries
                        .get(split)
                        .map(|entry| entry.offset.saturating_sub(read.start))
                        .unwrap_or(data.bytes.len() as u64)
                })
                .min(data.bytes.len() as u64) as usize;
            if before_end > 0 {
                chunks.push((read.file, read.start, data.bytes[..before_end].to_vec()));
            }
            if window {
                window_successful = true;
                channel.bytes_read += data.bytes.len() as u64;
                let sample = FormatStats::of(&data.bytes);
                if sample.format().has_unknown_content() {
                    snapshot.raw_blocks.push(RawBlock {
                        channel: index,
                        file: read.file.path.clone(),
                        text: String::from_utf8_lossy(&data.bytes).into_owned(),
                    });
                }
                stats.add(sample);
                if newest_entry
                    .as_ref()
                    .is_none_or(|(_, file)| read.file.mtime >= file.mtime)
                {
                    if let Some(entry_time) =
                        entries.iter().rev().find_map(|entry| entry.time.clone())
                    {
                        newest_entry = Some((entry_time, read.file));
                    }
                }
                if read.start > 0 {
                    let complete = match plan.cuts.get(&read.file.path) {
                        Some(cut) => read.start <= *cut,
                        None => entries
                            .first()
                            .and_then(|entry| entry.time.as_deref())
                            .is_some_and(|first| first < time.start_local.as_str()),
                    };
                    if !complete {
                        channel.complete = false;
                        channel.covered_from =
                            entries.get(split).and_then(|entry| entry.time.clone());
                    }
                }
                snapshot.entries.extend(entries.split_off(split));
            }
        }
        if window_successful {
            channel.format = stats.format();
        } else if plan.expected_window == 0 && !source.files.is_empty() {
            channel.format = Format::Empty;
        }
        // Newest pre-anchor chunks first, using file order and offsets rather
        // than ambiguous wall-clock times during a DST fall-back.
        chunks.sort_by_key(|(file, start, _)| {
            (
                source
                    .files
                    .iter()
                    .position(|candidate| candidate.path == file.path)
                    .unwrap_or(usize::MAX),
                std::cmp::Reverse(*start),
            )
        });
        let mut left = base_budget;
        let mut base_stats = FormatStats::default();
        for (file, start, bytes) in chunks {
            if left == 0 {
                channel.baseline.complete = false;
                break;
            }
            let count = (bytes.len() as u64).min(left) as usize;
            let at = start + bytes.len() as u64 - count as u64;
            if count < bytes.len() || at > 0 {
                channel.baseline.complete = false;
            }
            left -= count as u64;
            base_stats.add(FormatStats::of(&bytes[bytes.len() - count..]));
            let entries = parse_entries(&bytes[bytes.len() - count..], at, index, &file.path);
            if !entries.is_empty() {
                channel.baseline.available = true;
                channel.baseline.files.push(file.path.clone());
                for entry in &entries {
                    if let Some(at) = &entry.time {
                        if channel
                            .baseline
                            .covered_from
                            .as_ref()
                            .is_none_or(|first| at < first)
                        {
                            channel.baseline.covered_from = Some(at.clone());
                        }
                        if channel
                            .baseline
                            .covered_to
                            .as_ref()
                            .is_none_or(|last| at > last)
                        {
                            channel.baseline.covered_to = Some(at.clone());
                        }
                    }
                }
            }
            channel.baseline.bytes_read += count as u64;
            snapshot.baseline_entries.extend(entries);
        }
        if plan
            .cuts
            .values()
            .fold(0u64, |sum, size| sum.saturating_add(*size))
            > channel.baseline.bytes_read
        {
            channel.baseline.complete = false;
        }
        let format = base_stats.format();
        if format.has_unknown_content() {
            channel.baseline.available = false;
            channel.baseline.complete = false;
        }
        channel.baseline.format = Some(format);
        snapshot.channels.push(channel);
    }
    if let Some((entry_time, file)) = newest_entry {
        if let Some(warning) = clock_warning(&snapshot.zone, &entry_time, file, time.now) {
            snapshot.warnings.push(warning);
        }
    }
    Ok(snapshot)
}

/// What to look for: one file, or `<prefix>-YYYY-MM-DD.log` daily files.
#[derive(Debug, Clone)]
struct ChannelSource {
    name: String,
    path: String,
    daily: bool,
}

impl ChannelSource {
    fn from_target(target: &DeployTarget) -> Self {
        let path = target.log_path().to_string();
        let base = path.rsplit('/').next().unwrap_or(&path);
        let name = base.strip_suffix(".log").unwrap_or(base).to_string();
        Self {
            name,
            path,
            daily: target.log_daily,
        }
    }

    fn display(&self) -> String {
        if self.daily {
            format!("{}-YYYY-MM-DD.log", self.path)
        } else {
            self.path.clone()
        }
    }

    fn matches(&self, file: &ProbeFile) -> bool {
        if !self.daily {
            return file.path == self.path;
        }
        file.path
            .strip_prefix(&format!("{}-", self.path))
            .and_then(|path| path.strip_suffix(".log"))
            .is_some_and(is_date)
    }

    /// The files that can hold entries since `start_date`, newest first.
    #[cfg(test)]
    fn select<'a>(&self, files: &'a [ProbeFile], start_date: &str) -> Vec<&'a ProbeFile> {
        if !self.daily {
            return files.iter().filter(|file| self.matches(file)).collect();
        }
        let mut dated: Vec<(&str, &ProbeFile)> = files
            .iter()
            .filter_map(|file| {
                let date = file
                    .path
                    .strip_prefix(&format!("{}-", self.path))?
                    .strip_suffix(".log")?;
                (is_date(date) && date >= start_date).then_some((date, file))
            })
            .collect();
        dated.sort_by(|a, b| b.0.cmp(a.0));
        dated.into_iter().map(|(_, file)| file).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProbeFile {
    path: String,
    resolved: String,
    inode: u64,
    size: u64,
    mtime: u64,
    /// Modification time in the log's timezone.
    mtime_local: String,
    /// Resolves inside the app's `storage/logs`.
    inside: bool,
    /// Hash of up to 256 bytes at the probe-time end, to detect copytruncate.
    checksum: String,
    anchor_check: Option<(u64, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowTime {
    now: u64,
    start: u64,
    start_local: String,
    start_date: String,
    offset_start: String,
    offset_now: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProbeTime {
    Known(WindowTime),
    ZoneMissing,
    BadSince,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Probe {
    time: ProbeTime,
    files: Vec<ProbeFile>,
    anchor: anchor::Probe,
}

#[derive(Default)]
struct FormatStats {
    lines: u64,
    recognized: u64,
    headers: u64,
    unknown_file: bool,
}

/// For a numbered stack frame line like `#3 /srv/app/vendor/X.php(48): f()`,
/// its number and the text after the number.
pub fn frame_rest(line: &str) -> Option<(u64, &str)> {
    let rest = line.trim_start().strip_prefix('#')?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let after = rest[digits..].strip_prefix([' ', '\t'])?;
    Some((rest[..digits].parse().unwrap_or(u64::MAX), after))
}

/// A frame whose file sits under a whole `vendor` path segment. Lines that
/// are not numbered frames, or whose path can't be read, are not vendor.
pub fn vendor_frame(line: &str) -> bool {
    let Some((_, rest)) = frame_rest(line) else {
        return false;
    };
    let Some((path, _)) = rest.split_once('(') else {
        return false;
    };
    path.split(['/', '\\']).any(|segment| segment == "vendor")
}

impl FormatStats {
    fn of(bytes: &[u8]) -> Self {
        let mut stats = Self::default();
        for line in String::from_utf8_lossy(bytes).lines() {
            stats.lines += 1;
            let header = parse_header(line)
                .is_some_and(|header| normalize_time(&header.timestamp).is_some());
            let trimmed = line.trim();
            let continuation = line.starts_with([' ', '\t'])
                || frame_rest(trimmed).is_some()
                || matches!(
                    trimmed,
                    "[stacktrace]" | "[previous exception]" | "\"}" | "}"
                );
            stats.headers += u64::from(header);
            stats.recognized += u64::from(header || continuation);
        }
        stats
    }

    fn add(&mut self, other: Self) {
        self.unknown_file |= other.format().has_unknown_content();
        self.lines += other.lines;
        self.recognized += other.recognized;
        self.headers += other.headers;
    }

    fn format(&self) -> Format {
        if self.lines == 0 {
            return Format::Empty;
        }
        if self.headers == 0 {
            return Format::Unrecognized;
        }
        if !self.unknown_file && (self.lines < 20 || self.recognized * 2 >= self.lines) {
            Format::Recognized
        } else {
            Format::Partial(self.recognized * 100 / self.lines)
        }
    }
}

pub(crate) fn marker_candidates(target: &DeployTarget) -> (String, String) {
    candidate_commands(&ChannelSource::from_target(target), &target.logs)
}

fn candidate_commands(
    source: &ChannelSource,
    settings: &std::collections::BTreeMap<String, crate::LogChannel>,
) -> (String, String) {
    let mut files = String::new();
    for (key, settings) in settings {
        if let Some(path) = &settings.path {
            files.push_str(&format!(
                "if [ -e {path} ]; then emit {path} {key} || exit 1; fi\n",
                path = shell_quote(path),
                key = shell_quote(key)
            ));
        }
    }
    files.push_str(&if source.daily {
        format!(
            r#"prefix={}; for f in "$prefix"-*.log; do suffix=${{f#"$prefix"-}}; if [[ "$suffix" =~ ^[0-9]{{4}}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])\.log$ ]] && [ -e "$f" ]; then emit "$f" {} || exit 1; fi; done"#,
            shell_quote(&source.path),
            shell_quote(&source.name)
        )
    } else {
        format!(
            "if [ -e {path} ]; then emit {path} {key} || exit 1; fi",
            path = shell_quote(&source.path),
            key = shell_quote(&source.name)
        )
    });
    files.push_str(
        "\nfor f in storage/logs/*.log; do if [ -e \"$f\" ]; then emit \"$f\" || exit 1; fi; done",
    );
    let mut overrides = String::new();
    for (name, settings) in settings {
        if settings.hide {
            overrides.push_str(&format!("  {} ) return 0 ;;\n", shell_quote(name)));
        } else if let Some(path) = &settings.path {
            overrides.push_str(&format!(
                "  {} ) [ \"$1\" = {} ] || return 0 ;;\n",
                shell_quote(name),
                shell_quote(path)
            ));
        }
    }
    (files, overrides)
}

fn probe_script(
    app: &str,
    zone: &str,
    since: &Since,
    source: &ChannelSource,
    allow_outside: bool,
    settings: &std::collections::BTreeMap<String, crate::LogChannel>,
) -> String {
    let start = match since {
        Since::Ago(seconds) => format!("start=$((now - {seconds}))"),
        Since::At(at) => format!(
            "start=$(TZ=\"$tz\" date -d {} +%s 2>/dev/null) || {{ echo @bad-since; exit 0; }}",
            shell_quote(at)
        ),
    };
    let (files, overrides) = candidate_commands(source, settings);
    format!(
        r#"cd {app} || {{ echo "@unavailable cannot enter the app path"; exit 0; }}
for utility in date stat realpath dd sha256sum gzip base64; do
  command -v "$utility" >/dev/null || {{ echo "@unavailable server needs $utility"; exit 0; }}
done
tz={zone}
if [ ! -f "/usr/share/zoneinfo/$tz" ]; then echo @zone-missing; exit 0; fi
now=$(date +%s)
{start}
echo "@time $now $start $(TZ="$tz" date -d "@$start" '+%F %T %F %z') $(TZ="$tz" date +%z)"
{anchor_script}
root=$(realpath -e storage/logs 2>/dev/null)
set -o pipefail
seen=' '
emit() {{
  encoded=$(printf '%s' "$1" | base64 -w0)
  case "$seen" in *" $encoded "*) return 0 ;; esac
  seen="$seen$encoded "
  key=${{1##*/}}; key=${{key%.log}}
  if [[ "$key" =~ ^(.+)-[0-9]{{4}}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])$ ]]; then key=${{BASH_REMATCH[1]}}; fi
  key=${{2:-$key}}
  case "$key" in
{overrides}  *) ;; esac
  [ -f "$1" ] || return 0
  exec 3< "$1" 2>/dev/null || {{ echo '@unavailable log file is not readable'; return 0; }}
  fd="/proc/$$/fd/3"
  meta=$(stat -Lc '%i %s %Y' -- "$fd" 2>/dev/null) || return 1
  inside=0
  rp=$(realpath -e -- "$fd" 2>/dev/null) || return 1
  if [ -n "$root" ]; then case "$rp" in "$root"/*) inside=1 ;; esac; fi
  set -- "$1" $meta
  skip=$(( $3 > 256 ? $3 - 256 : 0 ))
  count=$(( $3 - skip ))
  checksum=unapproved
  if [ "$inside" -eq 1 ] || [ {allow_outside} -eq 1 ]; then
    checksum=$(dd if="$fd" iflag=skip_bytes,count_bytes skip="$skip" count="$count" status=none | sha256sum) || return 1
  fi
  resolved=$(printf '%s' "$rp" | base64 -w0)
  echo "@file $2 $3 $4 $inside $(TZ="$tz" date -d "@$4" '+%F %T') ${{checksum%% *}} $resolved"
  encoded_path=$(printf '%s' "$1" | base64 -w0)
  echo "$encoded_path"
  old_size=${{marker_offsets[$encoded_path]}}
  if [[ "$old_size" =~ ^[0-9]{{1,18}}$ ]] && [ "$2" = "${{marker_inodes[$encoded_path]}}" ] && [ "$3" -ge "$old_size" ] && {{ [ "$inside" -eq 1 ] || [ {allow_outside} -eq 1 ]; }}; then
    old_start=$(( old_size > 256 ? old_size - 256 : 0 ))
    old_sum=$(dd if="$fd" iflag=skip_bytes,count_bytes skip="$old_start" count="$(( old_size - old_start ))" status=none | sha256sum) || return 1
    echo "@split $encoded_path $old_size ${{old_sum%% *}}"
  fi
  exec 3<&-
}}
{files}
echo @end
"#,
        app = shell_quote(app),
        zone = shell_quote(zone),
        anchor_script = anchor::SCRIPT,
        allow_outside = u8::from(allow_outside),
    )
}

fn parse_probe(code: i32, lines: &[String]) -> Result<Probe, LogsError> {
    if code != 0 {
        return Err(LogsError::Unavailable(format!("probe exited with {code}")));
    }
    let engine = base64::engine::general_purpose::STANDARD;
    let mut time = None;
    let mut files = Vec::new();
    let mut anchor = anchor::Probe::default();
    let mut ended = false;
    let mut lines = lines.iter();
    let number = |value: &str| {
        value
            .parse::<u64>()
            .map_err(|_| LogsError::Unavailable("invalid numeric metadata in probe".into()))
    };
    while let Some(line) = lines.next() {
        if let Some(ProbeTime::Known(time)) = &time {
            if anchor.line(line, time) {
                continue;
            }
        }
        let mut fields = line.split(' ');
        match fields.next() {
            Some("@unavailable") => {
                return Err(LogsError::Unavailable(
                    line["@unavailable".len()..].trim().into(),
                ))
            }
            Some("@zone-missing") => time = Some(ProbeTime::ZoneMissing),
            Some("@bad-since") => time = Some(ProbeTime::BadSince),
            Some("@time") => {
                let f: Vec<&str> = fields.collect();
                let [now, start, date, clock, start_date, offset_start, offset_now] = f[..] else {
                    return Err(LogsError::Unavailable(format!(
                        "unexpected probe line: {line}"
                    )));
                };
                time = Some(ProbeTime::Known(WindowTime {
                    now: number(now)?,
                    start: number(start)?,
                    start_local: format!("{date} {clock}"),
                    start_date: start_date.into(),
                    offset_start: offset_start.into(),
                    offset_now: offset_now.into(),
                }));
            }
            Some("@file") => {
                let f: Vec<&str> = fields.collect();
                let path = lines
                    .next()
                    .and_then(|encoded| engine.decode(encoded).ok())
                    .and_then(|bytes| String::from_utf8(bytes).ok());
                let (&[inode, size, mtime, inside, date, clock, checksum, resolved], Some(path)) =
                    (&f[..], path)
                else {
                    return Err(LogsError::Unavailable(
                        "invalid file metadata in probe".into(),
                    ));
                };
                files.push(ProbeFile {
                    path,
                    resolved: engine
                        .decode(resolved)
                        .ok()
                        .and_then(|bytes| String::from_utf8(bytes).ok())
                        .ok_or_else(|| {
                            LogsError::Unavailable("invalid resolved file path in probe".into())
                        })?,
                    inode: number(inode)?,
                    size: number(size)?,
                    mtime: number(mtime)?,
                    mtime_local: format!("{date} {clock}"),
                    inside: inside == "1",
                    checksum: checksum.into(),
                    anchor_check: None,
                });
            }
            Some("@end") => ended = true,
            _ => {}
        }
    }
    match time {
        Some(ProbeTime::Known(_)) if !ended => {
            Err(LogsError::Unavailable("probe output ended early".into()))
        }
        Some(time) => Ok(Probe {
            time,
            files,
            anchor,
        }),
        None => Err(LogsError::Unavailable("probe printed no time".into())),
    }
}

/// Splits `total` bytes between channels: each gets what it needs up to `cap`;
/// when that does not fit, channels share equally and small ones hand their
/// unused share to the rest. No channel with data gets less than `floor`.
fn water_fill(needs: &[u64], total: u64, cap: u64, floor: u64) -> Vec<u64> {
    let wanted: Vec<u64> = needs.iter().map(|need| (*need).min(cap)).collect();
    let mut given = vec![0; needs.len()];
    let mut open: Vec<usize> = (0..needs.len()).filter(|i| wanted[*i] > 0).collect();
    let mut left = total;
    while !open.is_empty() {
        let share = left / open.len() as u64;
        let (small, large): (Vec<usize>, Vec<usize>) =
            open.iter().partition(|i| wanted[**i] <= share);
        if small.is_empty() {
            for i in &large {
                given[*i] = share;
            }
            break;
        }
        for i in small {
            given[i] = wanted[i];
            left -= wanted[i];
        }
        open = large;
    }
    let can_fit_floor = floor.saturating_mul(needs.len() as u64) <= total;
    for (i, amount) in given.iter_mut().enumerate() {
        if wanted[i] > 0 && can_fit_floor {
            *amount = (*amount).max(floor.min(wanted[i]));
        }
    }
    given
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReadPlan<'a> {
    file: &'a ProbeFile,
    start: u64,
    count: u64,
}

/// Newest files first, each read from its end, until `budget` runs out.
#[cfg(test)]
fn plan_reads<'a>(files: &[&'a ProbeFile], budget: u64) -> Vec<ReadPlan<'a>> {
    let mut left = budget;
    let mut plan = Vec::new();
    for file in files {
        if left == 0 && file.size > 0 {
            continue;
        }
        let count = file.size.min(left);
        left -= count;
        plan.push(ReadPlan {
            file,
            start: file.size - count,
            count,
        });
    }
    plan
}

fn read_script(app: &str, plan: &[ReadPlan], allow_outside: bool) -> String {
    let outside = if allow_outside {
        ""
    } else {
        "\n  if [ -z \"$root\" ]; then ok=0; else case \"$rp\" in \"$root\"/*) ;; *) ok=0 ;; esac; fi"
    };
    let mut script = format!(
        "cd {} || exit 1\nset -o pipefail\nroot=$(realpath -e storage/logs 2>/dev/null)\n",
        shell_quote(app)
    );
    for (index, read) in plan.iter().enumerate() {
        let anchor_check = read.file.anchor_check.as_ref().map(|(offset, checksum)| format!("sum=$(dd if=\"$fd\" iflag=skip_bytes,count_bytes skip={} count={} status=none | sha256sum) || return 1\n  [ \"${{sum%% *}}\" = {} ]", offset.saturating_sub(256), (*offset).min(256), shell_quote(checksum))).unwrap_or_else(|| ":".into());
        script.push_str(&format!(
            r#"f={path}
ok=1
[ -f "$f" ] && {{ exec 3< "$f"; }} 2>/dev/null || ok=0
fd="/proc/$$/fd/3"
rp=$(realpath -e -- "$fd" 2>/dev/null) || ok=0{outside}
meta=$(stat -Lc '%i %s' -- "$fd" 2>/dev/null) || ok=0
if [ $ok -eq 1 ]; then set -- $meta; [ "$1" = {inode} ] && [ "$2" -ge {size} ] || ok=0; fi
check_end() {{
  sum=$(dd if="$fd" iflag=skip_bytes,count_bytes skip={check_start} count={check_count} status=none | sha256sum) || return 1
  [ "${{sum%% *}}" = {checksum} ] || return 1
  {anchor_check}
}}
if [ $ok -eq 1 ]; then check_end || ok=0; fi
if [ $ok -eq 1 ]; then
  echo "@data {index}"
  dd if="$fd" iflag=skip_bytes,count_bytes skip={start} count={count} status=none | gzip -c | base64 -w0 || exit 1
  echo
  check_end || echo "@changed {index}"
else
  echo "@changed {index}"
fi
exec 3<&-
"#,
            path = shell_quote(&read.file.path),
            inode = read.file.inode,
            size = read.file.size,
            start = read.start,
            count = read.count,
            check_start = read.file.size.saturating_sub(256),
            check_count = read.file.size.min(256),
            checksum = shell_quote(&read.file.checksum),
        ));
    }
    script.push_str("echo @end\n");
    script
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReadData {
    bytes: Vec<u8>,
    transferred: u64,
}

/// One result per planned file: its bytes, or `None` if it changed.
fn parse_read(
    code: i32,
    lines: &[String],
    counts: &[u64],
) -> Result<Vec<Option<ReadData>>, LogsError> {
    if code != 0 {
        return Err(LogsError::Unavailable(format!(
            "log read exited with {code}"
        )));
    }
    let engine = base64::engine::general_purpose::STANDARD;
    let files = counts.len();
    let mut results = vec![None; files];
    let mut seen = vec![false; files];
    let mut ended = false;
    let mut lines = lines.iter();
    while let Some(line) = lines.next() {
        let mut fields = line.split(' ');
        let (kind, index) = (
            fields.next(),
            fields.next().and_then(|i| i.parse::<usize>().ok()),
        );
        match (kind, index) {
            (Some("@data"), Some(index)) if index < files => {
                let encoded = lines.next().map(String::as_str).unwrap_or("");
                let compressed = engine
                    .decode(encoded)
                    .map_err(|error| LogsError::Unavailable(format!("bad log data: {error}")))?;
                let mut bytes = Vec::new();
                flate2::read::GzDecoder::new(compressed.as_slice())
                    .take(counts[index].saturating_add(1))
                    .read_to_end(&mut bytes)
                    .map_err(|error| LogsError::Unavailable(format!("bad log data: {error}")))?;
                if bytes.len() as u64 != counts[index] {
                    return Err(LogsError::Unavailable(
                        "log data did not match the requested byte count".into(),
                    ));
                }
                results[index] = Some(ReadData {
                    bytes,
                    transferred: encoded.len() as u64,
                });
                seen[index] = true;
            }
            (Some("@changed"), Some(index)) if index < files => {
                results[index] = None;
                seen[index] = true;
            }
            (Some("@end"), _) => ended = true,
            _ => {}
        }
    }
    if !ended || seen.contains(&false) {
        return Err(LogsError::Unavailable("log read ended early".into()));
    }
    Ok(results)
}

/// Splits bytes read from `file` at byte `start` into entries. A read that
/// starts mid-file skips lines until the first entry header.
fn parse_entries(bytes: &[u8], start: u64, channel: usize, file: &str) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut offset = start;
    let mut truncated = false;
    for raw in bytes.split_inclusive(|byte| *byte == b'\n') {
        let line_offset = offset;
        offset += raw.len() as u64;
        let line = String::from_utf8_lossy(raw);
        let content = line.strip_suffix('\n').unwrap_or(&line);
        if let Some(header) =
            parse_header(content).filter(|header| normalize_time(&header.timestamp).is_some())
        {
            let level = Level::parse(&header.level).unwrap_or(Level::Debug);
            let text = clip_utf8(content, ENTRY_BYTES);
            let message_start = (content.len() - header.message.len()).min(text.len());
            entries.push(Entry {
                channel,
                file: file.into(),
                offset: line_offset,
                time: normalize_time(&header.timestamp),
                level,
                text,
                truncated: content.len() > ENTRY_BYTES,
                message_start,
            });
            truncated = false;
        } else if let Some(entry) = entries.last_mut() {
            if truncated || entry.text.len() + content.len() >= ENTRY_BYTES {
                truncated = true;
                entry.truncated = true;
                continue;
            }
            entry.text.push('\n');
            entry.text.push_str(content);
        }
    }
    entries
}

/// `2026-10-01 14:00:00` from Laravel's format or an ISO 8601 timestamp.
fn normalize_time(raw: &str) -> Option<String> {
    let head = raw.get(..19)?;
    if !head.is_ascii() {
        return None;
    }
    let normalized = head.replacen('T', " ", 1);
    (is_date(&normalized[..10])
        && normalized.as_bytes()[10] == b' '
        && is_clock(&normalized[11..16])
        && normalized.as_bytes()[16] == b':'
        && normalized[17..].parse::<u32>().is_ok_and(|s| s < 61))
    .then_some(normalized)
}

/// Drops entries before the first one at or after `start`. Later entries count
/// whatever their time says, so a repeated DST hour is kept.
#[cfg(test)]
fn in_window(entries: Vec<Entry>, start: &str) -> Vec<Entry> {
    let first = entries
        .iter()
        .position(|entry| entry.time.as_deref().is_none_or(|time| time >= start));
    match first {
        Some(first) => entries.into_iter().skip(first).collect(),
        None => Vec::new(),
    }
}

/// Warns when the newest entry's time, read in `zone`, is far from the time
/// its file was last written. Skipped for files idle for over a day.
fn clock_warning(zone: &str, entry_time: &str, file: &ProbeFile, now: u64) -> Option<String> {
    if now.saturating_sub(file.mtime) > 86_400 {
        return None;
    }
    let skew = naive_seconds(entry_time)? - naive_seconds(&file.mtime_local)?;
    (skew.abs() > CLOCK_SKEW_SECS).then(|| {
        format!(
            "log times don't look like {zone} (newest entry {entry_time}, file written {} {zone}); \
             set `timezone` in .shipslip.toml",
            file.mtime_local
        )
    })
}

/// Seconds since 1970-01-01 for a local `YYYY-MM-DD HH:MM:SS`, ignoring zones.
fn naive_seconds(time: &str) -> Option<i64> {
    let normalized = normalize_time(time)?;
    let number = |range: std::ops::Range<usize>| normalized[range].parse::<i64>().ok();
    let (y, m, d) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + number(11..13)? * 3600 + number(14..16)? * 60 + number(17..19)?)
}

/// `YYYY-MM-DD HH:MM:SS` for Unix `epoch` seconds in a zone whose `date +%z`
/// offset is `offset`, like `+0530`.
fn local_time(epoch: u64, offset: &str) -> Option<String> {
    let (sign, digits) = match offset.as_bytes().first()? {
        b'+' => (1, &offset[1..]),
        b'-' => (-1, &offset[1..]),
        _ => return None,
    };
    if digits.len() != 4 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let shift =
        sign * (digits[..2].parse::<i64>().ok()? * 3600 + digits[2..].parse::<i64>().ok()? * 60);
    let seconds = i64::try_from(epoch).ok()? + shift;
    let (days, clock) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    Some(format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        clock / 3600,
        clock % 3600 / 60,
        clock % 60
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Badge {
    New,
    Unknown,
    Seen,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    pub badge: Badge,
    signature: String,
    channels: Vec<usize>,
    pub message: String,
    pub file_line: Option<String>,
    pub count: u64,
}

/// Entries that share an exception class and app file, or, when they name
/// neither, a normalized message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub badge: Badge,
    pub id: String,
    pub(crate) full_id: String,
    pub class: String,
    pub file: Option<String>,
    pub(crate) normalized_message: Option<String>,
    pub count: u64,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    /// The first line of the latest entry, clipped.
    pub message: String,
    /// Index of the latest entry in `Snapshot::entries`.
    pub latest: usize,
    first: usize,
    pub channels: Vec<usize>,
    pub variants: Vec<Variant>,
    pub overflow_variants: u64,
    /// One of its channels was not fully read or understood, so `count` is a
    /// lower bound.
    pub partial: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Groups {
    /// Most frequent first.
    pub groups: Vec<Group>,
    /// Groups past `MAX_GROUPS`, counted but not tracked.
    pub overflow: u64,
}

/// Groups entries at `min_level` or above whose text contains `grep`.
pub fn group(snapshot: &Snapshot, min_level: Level, grep: Option<&str>) -> Groups {
    let grep = grep.map(str::to_lowercase);
    let mut index: HashMap<GroupKey, usize> = HashMap::new();
    let mut overflow = HashMap::new();
    let mut groups: Vec<Group> = Vec::new();
    let mut variant_index: Vec<HashMap<String, usize>> = Vec::new();
    for (position, entry) in snapshot.entries.iter().enumerate() {
        if entry.level < min_level || !matches_grep(entry, grep.as_deref()) {
            continue;
        }
        let entry_signature = signature(&entry.level.name().to_ascii_uppercase(), entry.message());
        let key = entry_signature.group_key();
        let EntrySignature {
            signature,
            class,
            file,
            display,
            normalized_message,
        } = entry_signature;
        let slot = match index.get(&key) {
            Some(slot) => *slot,
            None if groups.len() < MAX_GROUPS => {
                index.insert(key, groups.len());
                variant_index.push(HashMap::new());
                groups.push(Group {
                    badge: Badge::Seen,
                    id: String::new(),
                    full_id: group_id(&class, file.as_deref(), normalized_message.as_deref()),
                    class,
                    file,
                    normalized_message,
                    count: 0,
                    first_seen: None,
                    last_seen: None,
                    message: String::new(),
                    latest: position,
                    first: position,
                    channels: Vec::new(),
                    variants: Vec::new(),
                    overflow_variants: 0,
                    partial: false,
                });
                groups.len() - 1
            }
            None => {
                overflow.insert(key, ());
                continue;
            }
        };
        let group = &mut groups[slot];
        group.count += 1;
        if let Some(time) = &entry.time {
            let first = &snapshot.entries[group.first];
            let is_first = if entry.file == first.file {
                entry.offset <= first.offset
            } else {
                group.first_seen.as_ref().is_none_or(|first| time < first)
            };
            if is_first {
                group.first_seen = Some(time.clone());
                group.first = position;
            }
            let latest = &snapshot.entries[group.latest];
            let is_latest = if entry.file == latest.file {
                entry.offset >= latest.offset
            } else {
                group.last_seen.as_ref().is_none_or(|last| time >= last)
            };
            if is_latest {
                group.last_seen = Some(time.clone());
                group.latest = position;
            }
        } else if group.last_seen.is_none() {
            group.latest = position;
        }
        if !group.channels.contains(&entry.channel) {
            group.channels.push(entry.channel);
            group.partial |= channel_incomplete(&snapshot.channels[entry.channel]);
        }
        let first_line = entry.message().lines().next().unwrap_or("");
        match variant_index[slot].get(&signature) {
            Some(variant) => {
                group.variants[*variant].count += 1;
                if !group.variants[*variant].channels.contains(&entry.channel) {
                    group.variants[*variant].channels.push(entry.channel);
                }
            }
            None if group.variants.len() < MAX_VARIANTS => {
                variant_index[slot].insert(signature.clone(), group.variants.len());
                group.variants.push(Variant {
                    badge: Badge::Seen,
                    signature,
                    channels: vec![entry.channel],
                    message: clip_utf8(without_context(first_line), MESSAGE_BYTES),
                    file_line: display,
                    count: 1,
                });
            }
            None => group.overflow_variants += 1,
        }
    }
    let mut baseline_keys = HashSet::new();
    let mut baseline_signatures = HashSet::new();
    for entry in &snapshot.baseline_entries {
        let entry_signature = signature(&entry.level.name().to_ascii_uppercase(), entry.message());
        baseline_keys.insert(entry_signature.group_key());
        baseline_signatures.insert(entry_signature.signature);
    }
    let badge = |seen: bool, channels: &[usize]| {
        if !snapshot.baseline_enabled || seen {
            Badge::Seen
        } else if channels
            .iter()
            .any(|channel| snapshot.channels[*channel].baseline.available)
        {
            Badge::New
        } else {
            Badge::Unknown
        }
    };
    for group in &mut groups {
        group.badge = badge(
            baseline_keys.contains(&(
                group.class.clone(),
                group.file.clone(),
                group.normalized_message.clone(),
            )),
            &group.channels,
        );
        for variant in &mut group.variants {
            variant.badge = badge(
                baseline_signatures.contains(&variant.signature),
                &variant.channels,
            );
        }
        let latest = &snapshot.entries[group.latest];
        group.message = clip_utf8(
            without_context(latest.message().lines().next().unwrap_or("")),
            MESSAGE_BYTES,
        );
        group
            .variants
            .sort_by_key(|variant| std::cmp::Reverse(variant.count));
    }
    groups.sort_by(|a, b| {
        a.badge
            .cmp(&b.badge)
            .then_with(|| b.count.cmp(&a.count))
            .then_with(|| b.last_seen.cmp(&a.last_seen))
            .then_with(|| a.full_id.cmp(&b.full_id))
    });
    assign_ids(&mut groups);
    Groups {
        groups,
        overflow: overflow.len() as u64,
    }
}

/// Candidate JSON starts tried from the end of a line, so a long line can't
/// make the search slow.
const CONTEXT_TRIES: usize = 8;

/// The first line of a message without the context and extra JSON that
/// Laravel's formatter appends, for a short label. A Laravel exception's
/// context starts on this line and closes lines later; any other context
/// must be one or two complete JSON objects or arrays at the end.
pub(crate) fn without_context(line: &str) -> &str {
    if let Some(key) = line.find("\"exception\":\"[object] (") {
        if let Some(start) = line[..key].rfind(" {") {
            return line[..start].trim_end();
        }
    }
    let mut cut = None;
    let starts = line
        .match_indices(' ')
        .map(|(at, _)| at)
        .filter(|at| matches!(line.as_bytes().get(at + 1), Some(b'{' | b'[')))
        .collect::<Vec<_>>();
    for &at in starts.iter().rev().take(CONTEXT_TRIES) {
        if json_tail(&line[at + 1..]) {
            cut = Some(at);
        }
    }
    cut.map_or(line, |at| line[..at].trim_end())
}

/// An entry's first message line without Laravel's context and extra JSON.
pub fn headline(entry: &Entry) -> &str {
    without_context(entry.message().lines().next().unwrap_or(""))
}

/// `text` is one or two JSON objects or arrays and nothing else.
fn json_tail(text: &str) -> bool {
    let mut values = 0;
    for value in serde_json::Deserializer::from_str(text).into_iter::<serde_json::Value>() {
        match value {
            Ok(serde_json::Value::Object(_) | serde_json::Value::Array(_)) if values < 2 => {
                values += 1
            }
            _ => return false,
        }
    }
    values > 0
}

/// Context keys that name one request or job, narrowest first.
pub const ID_KEYS: [&str; 5] = [
    "request_id",
    "trace_id",
    "correlation_id",
    "job_id",
    "batch_id",
];
/// Trailing JSON larger than this is not read for IDs (exception context aside).
const ID_TAIL_BYTES: usize = 16 * 1024;
/// Candidate JSON starts tried from the end when there is no exception context.
const ID_TRIES: usize = 4;

/// An ID read from the top level of an entry's context or extra JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestId {
    pub key: &'static str,
    pub value: String,
    /// Context and extra both had this key with different values; context won.
    pub conflict: bool,
}

/// The context and extra objects Laravel appended to `entry`, parsed as JSON
/// after re-escaping the line breaks Monolog leaves inside exception strings.
fn context_objects(entry: &Entry) -> Vec<serde_json::Map<String, serde_json::Value>> {
    let tail = entry.message();
    let start = match tail.find("\"exception\":\"[object] (") {
        Some(key) => tail[..key].rfind(" {").map(|at| vec![at]),
        None => None,
    };
    let starts = start.unwrap_or_else(|| {
        let mut starts: Vec<usize> = tail
            .match_indices(' ')
            .map(|(at, _)| at)
            .filter(|at| matches!(tail.as_bytes().get(at + 1), Some(b'{' | b'[')))
            .filter(|at| tail.len() - at <= ID_TAIL_BYTES)
            .collect();
        starts.reverse();
        starts.truncate(ID_TRIES);
        starts.reverse();
        starts
    });
    for at in starts {
        let text = tail[at + 1..]
            .replace('\r', "\\r")
            .replace('\n', "\\n")
            .replace('\t', "\\t");
        let mut objects = Vec::new();
        let mut ok = true;
        for value in serde_json::Deserializer::from_str(&text).into_iter::<serde_json::Value>() {
            match value {
                Ok(serde_json::Value::Object(map)) if objects.len() < 2 => objects.push(map),
                Ok(serde_json::Value::Array(items)) if items.is_empty() && objects.len() < 2 => {
                    objects.push(serde_json::Map::new())
                }
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && !objects.is_empty() {
            return objects;
        }
    }
    Vec::new()
}

fn id_value(key: &str, value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text)
            if (8..=128).contains(&text.len())
                && text
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte)) =>
        {
            Some(text.clone())
        }
        serde_json::Value::Number(number)
            if matches!(key, "job_id" | "batch_id") && number.is_u64() =>
        {
            Some(number.to_string())
        }
        _ => None,
    }
}

/// The narrowest request or job ID in `entry`'s context or extra, if any.
pub fn request_id(entry: &Entry) -> Option<RequestId> {
    let objects = context_objects(entry);
    for key in ID_KEYS {
        let values: Vec<String> = objects
            .iter()
            .filter_map(|object| object.get(key).and_then(|value| id_value(key, value)))
            .collect();
        if let Some(first) = values.first() {
            return Some(RequestId {
                key,
                value: first.clone(),
                conflict: values.iter().any(|value| value != first),
            });
        }
    }
    None
}

/// Entries that share a request or job ID with one entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Related {
    pub id: RequestId,
    /// Entry indices before and after the chosen entry, in time order.
    pub before: Vec<usize>,
    pub after: Vec<usize>,
    /// Some channel was not fully read or understood, so lines may be missing.
    pub partial: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Correlation {
    Shared(Related),
    /// The entry has no usable ID.
    NoId,
    /// The entry was cut at the size limit before any ID was found.
    Cut,
}

/// Other entries in the window that share `entry`'s request or job ID.
/// This is correlation, not cause: it says the lines came from the same
/// request, not that one led to the other.
pub fn related(snapshot: &Snapshot, entry: usize) -> Correlation {
    let chosen = &snapshot.entries[entry];
    let Some(id) = request_id(chosen) else {
        return if chosen.truncated {
            Correlation::Cut
        } else {
            Correlation::NoId
        };
    };
    let mut matches: Vec<usize> = snapshot
        .entries
        .iter()
        .enumerate()
        .filter(|(index, other)| {
            *index == entry
                || (other.text.contains(&id.value)
                    && request_id(other)
                        .is_some_and(|other| other.key == id.key && other.value == id.value))
        })
        .map(|(index, _)| index)
        .collect();
    matches.sort_by(|a, b| {
        let (a_time, b_time) = (&snapshot.entries[*a].time, &snapshot.entries[*b].time);
        a_time
            .is_none()
            .cmp(&b_time.is_none())
            .then_with(|| a_time.cmp(b_time))
            .then_with(|| a.cmp(b))
    });
    let at = matches
        .iter()
        .position(|index| *index == entry)
        .unwrap_or(0);
    Correlation::Shared(Related {
        before: matches[..at].to_vec(),
        after: matches[at + 1..].to_vec(),
        partial: coverage_incomplete(snapshot),
        id,
    })
}

fn matches_grep(entry: &Entry, grep: Option<&str>) -> bool {
    grep.is_none_or(|grep| entry.text.to_lowercase().contains(grep))
}

/// A stable ID for a group's key: the same group gets the same ID on
/// every run, so an ID printed earlier can be looked up again.
fn group_id(class: &str, file: Option<&str>, normalized_message: Option<&str>) -> String {
    let mut key = format!("{class}\0{}", file.unwrap_or(""));
    if let Some(message) = normalized_message {
        key.push('\0');
        key.push_str(message);
    }
    let digest = Sha256::digest(key);
    digest
        .iter()
        .take(16)
        .map(|byte| ID_ALPHABET[*byte as usize % ID_ALPHABET.len()] as char)
        .collect()
}

/// The shortest prefix length, at least `ID_LEN`, that tells all groups apart.
fn assign_ids(groups: &mut [Group]) {
    let full_len = groups.first().map_or(ID_LEN, |group| group.full_id.len());
    let len = (ID_LEN..full_len)
        .find(|len| {
            let mut seen = std::collections::HashSet::new();
            groups
                .iter()
                .all(|group| seen.insert(&group.full_id[..*len]))
        })
        .unwrap_or(full_len);
    for group in groups {
        group.id = group.full_id[..len].to_string();
    }
}

pub enum Lookup<'a> {
    Found(&'a Group),
    Ambiguous(Vec<&'a Group>),
    Missing,
}

/// Finds a group by row number or by ID prefix, ignoring case.
pub fn find<'a>(groups: &'a Groups, query: &str) -> Lookup<'a> {
    if !query.is_empty() && query.chars().all(|c| c.is_ascii_digit()) {
        return match query
            .parse::<usize>()
            .ok()
            .and_then(|row| row.checked_sub(1))
        {
            Some(row) if row < groups.groups.len() => Lookup::Found(&groups.groups[row]),
            _ => Lookup::Missing,
        };
    }
    let query = query.to_ascii_lowercase();
    let matches: Vec<&Group> = groups
        .groups
        .iter()
        .filter(|group| !query.is_empty() && group.full_id.starts_with(&query))
        .collect();
    match matches.len() {
        0 => Lookup::Missing,
        1 => Lookup::Found(matches[0]),
        _ => Lookup::Ambiguous(matches),
    }
}

/// Escapes control characters other than newline and tab, so log content
/// cannot move the cursor, recolor, or retitle the terminal. Bidirectional
/// formatting characters are escaped too, so text cannot appear reordered.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' | '\t' => out.push(c),
            c if c.is_control() || is_bidi_format(c) => {
                let code = c as u32;
                if code < 0x100 {
                    out.push_str(&format!("\\x{code:02x}"));
                } else {
                    out.push_str(&format!("\\u{{{code:x}}}"));
                }
            }
            c => out.push(c),
        }
    }
    out
}

fn is_bidi_format(c: char) -> bool {
    matches!(
        c,
        '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

/// Like [`escape`], but also escapes newlines and tabs, for text that must
/// stay on one line, such as a table cell or a labeled field.
pub fn escape_field(text: &str) -> String {
    escape(text).replace('\n', "\\x0a").replace('\t', "\\x09")
}

/// Some of this channel's window was not read, or was read but not understood.
fn channel_incomplete(channel: &Channel) -> bool {
    !channel.complete || channel.files.is_empty() || channel.format.has_unknown_content()
}

/// Entries in the window may be missing from what was read, so every count
/// is a lower bound.
pub fn coverage_incomplete(snapshot: &Snapshot) -> bool {
    snapshot.channels.is_empty() || snapshot.channels.iter().any(channel_incomplete)
}

/// Entries in the window by level, most severe first, ignoring any filter.
/// Levels with no entries are left out.
pub fn level_counts(snapshot: &Snapshot) -> Vec<(Level, u64)> {
    let mut counts = [0u64; Level::ALL.len()];
    for entry in &snapshot.entries {
        counts[entry.level as usize] += 1;
    }
    Level::ALL
        .iter()
        .rev()
        .filter(|level| counts[**level as usize] > 0)
        .map(|level| (*level, counts[*level as usize]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_badges_use_global_baselines_and_variant_channel_coverage() {
        let mut snapshot = snapshot_of("[2026-10-02 12:00:00] production.ERROR: KnownException: changed\n[2026-10-02 12:00:01] production.ERROR: NewException: new\n", true);
        snapshot.baseline_enabled = true;
        snapshot.channels[0].baseline.available = true;
        let mut missing = snapshot.channels[0].clone();
        missing.name = "queue".into();
        missing.baseline.available = false;
        snapshot.channels.push(missing);
        snapshot.entries.extend(parse_entries(
            b"[2026-10-02 12:00:02] production.ERROR: MissingException: no baseline\n",
            0,
            1,
            "queue.log",
        ));
        snapshot.baseline_entries = parse_entries(
            b"[2026-10-01 12:00:00] production.ERROR: KnownException: previous\n",
            0,
            1,
            "queue.log",
        );
        let groups = group(&snapshot, Level::Error, None);
        assert_eq!(
            groups
                .groups
                .iter()
                .map(|group| group.badge)
                .collect::<Vec<_>>(),
            [Badge::New, Badge::Unknown, Badge::Seen]
        );
        let known = groups
            .groups
            .iter()
            .find(|group| group.class == "KnownException")
            .unwrap();
        assert_eq!(known.badge, Badge::Seen);
        assert_eq!(known.variants[0].badge, Badge::New);
    }

    #[test]
    fn a_group_merges_channels_and_retains_first_and_latest_times() {
        let mut snapshot = snapshot_of(
            "[2026-10-02 12:00:00] production.ERROR: QueryException: newer\n",
            true,
        );
        let mut second = snapshot.channels[0].clone();
        second.name = "payments".into();
        second.complete = false;
        snapshot.channels.push(second);
        snapshot.entries.extend(parse_entries(
            b"[2026-10-01 12:00:00] production.ERROR: QueryException: older\n",
            0,
            1,
            "payments.log",
        ));
        let groups = group(&snapshot, Level::Error, None);
        assert_eq!(groups.groups.len(), 1);
        let group = &groups.groups[0];
        assert_eq!(group.count, 2);
        assert_eq!(group.channels, [0, 1]);
        assert_eq!(group.first_seen.as_deref(), Some("2026-10-01 12:00:00"));
        assert_eq!(group.last_seen.as_deref(), Some("2026-10-02 12:00:00"));
        assert!(group.partial);
    }

    #[test]
    fn format_detection_handles_traces_quiet_channels_and_mixed_output() {
        let header = entry_line("2026-10-01 01:00:00", "ERROR", "RuntimeException: boom");
        assert_eq!(
            FormatStats::of(header.as_bytes()).format(),
            Format::Recognized
        );
        let trace = format!("{header}[stacktrace]\n{}", "#0 frame()\n".repeat(30));
        assert_eq!(
            FormatStats::of(trace.as_bytes()).format(),
            Format::Recognized
        );
        let mixed = format!("{}{}{}", "worker stdout\n".repeat(40), header, trace);
        assert!(matches!(
            FormatStats::of(mixed.as_bytes()).format(),
            Format::Partial(_)
        ));
        assert_eq!(
            FormatStats::of(b"{\"message\":\"boom\",\"level_name\":\"ERROR\"}\n").format(),
            Format::Unrecognized
        );
        assert_eq!(
            FormatStats::of(b"#0 orphaned trace\n").format(),
            Format::Unrecognized
        );
        assert_eq!(FormatStats::of(b"").format(), Format::Empty);
    }

    #[test]
    fn raw_fallback_preserves_unknown_text_escapes_controls_and_applies_grep() {
        let text = format!(
            "plain worker stdout\n{}",
            entry_line("2026-10-01 01:00:00", "ERROR", "RuntimeException: boom")
        );
        let mut snapshot = snapshot_of(&text, true);
        snapshot.channels[0].format = Format::Partial(10);
        snapshot.raw_blocks.push(RawBlock {
            channel: 0,
            file: "worker.log".into(),
            text: "ordinary stdout\nPayment failed\x1b]0;secret\x07\n".into(),
        });
        assert_eq!(group(&snapshot, Level::Error, None).groups.len(), 1);
    }

    #[test]
    fn short_paths_handle_relative_logs_without_changing_group_identity() {
        let path = "/var/www/laravel/app/Services/Payment.php";
        let mut snapshot = snapshot_of(
            &entry_line(
                "2026-10-01 01:00:00",
                "ERROR",
                &format!("PaymentException: failed at {path}:42"),
            ),
            true,
        );
        snapshot.channels[0].files = vec!["storage/logs/laravel.log".into()];
        let groups = group(&snapshot, Level::Error, None);
        assert_eq!(groups.groups[0].file.as_deref(), Some(path));
    }

    fn file(path: &str, size: u64) -> ProbeFile {
        ProbeFile {
            path: path.into(),
            resolved: path.into(),
            inode: 7,
            size,
            mtime: 1_000,
            mtime_local: "2026-10-01 10:00:00".into(),
            inside: true,
            checksum: "unused".into(),
            anchor_check: None,
        }
    }

    fn entry_line(time: &str, level: &str, message: &str) -> String {
        format!("[{time}] production.{level}: {message}\n")
    }

    fn snapshot_of(text: &str, complete: bool) -> Snapshot {
        Snapshot {
            env: "production".into(),
            zone: "UTC".into(),
            anchor: Anchor::Default {
                seconds: DEFAULT_WINDOW,
            },
            anchor_fallbacks: Vec::new(),
            start: "2026-10-01 00:00:00".into(),
            read_at: Some("2026-10-02 00:00:00".into()),
            channels: vec![Channel {
                name: "laravel".into(),
                key: "laravel".into(),
                kind: ChannelKind::Single,
                source: "storage/logs/laravel.log".into(),
                files: vec!["/srv/app/storage/logs/laravel.log".into()],
                budget: CHANNEL_BYTES,
                bytes_read: text.len() as u64,
                complete,
                covered_from: (!complete).then(|| "2026-10-01 09:00:00".into()),
                changed: Vec::new(),
                size: text.len() as u64,
                last_write: Some("2026-10-01 10:00:00".into()),
                format: Format::Recognized,
                omitted_files: 0,
                baseline: Baseline::default(),
            }],
            entries: parse_entries(text.as_bytes(), 0, 0, "/srv/app/storage/logs/laravel.log"),
            raw_blocks: Vec::new(),
            baseline_entries: Vec::new(),
            baseline_enabled: false,
            clock_changed: false,
            warnings: Vec::new(),
            bytes_transferred: 0,
        }
    }

    #[test]
    fn exception_groups_decode_names_and_keep_raw_entries() {
        let class = "Illuminate\\Database\\QueryException";
        let file = "/srv/app/app/Http/Controllers/InvoiceExportController.php";
        let context = serde_json::json!({"exception": format!("[object] ({class}(code: HY000): Missing table at {file}:16)")});
        let encoded = entry_line(
            "2026-10-01 02:00:00",
            "ERROR",
            &format!("Missing table {context}"),
        );
        let plain = entry_line(
            "2026-10-01 01:00:00",
            "ERROR",
            &format!("{class}: Missing table at {file}:16"),
        );
        let mut snapshot = snapshot_of(&format!("{plain}{encoded}"), true);
        snapshot.baseline_enabled = true;
        snapshot.channels[0].baseline.available = true;
        snapshot.baseline_entries = parse_entries(encoded.as_bytes(), 0, 0, "laravel.log");
        let groups = group(&snapshot, Level::Error, None);
        assert_eq!(groups.groups.len(), 1);
        let g = &groups.groups[0];
        assert_eq!(g.class, class);
        assert_eq!(g.count, 2);
        assert_eq!(g.badge, Badge::Seen);
        assert_eq!(g.id, &group_id(class, Some(file), None)[..ID_LEN]);
    }

    #[test]
    fn exception_groups_ignore_middleware_for_identity_and_baseline() {
        let header = "Class not found {\"exception\":\"[object] (Error(code: 0): Class not found at /srv/app/app/Http/Controllers/InvoiceExportController.php:16)\n[stacktrace]\n";
        let baseline = entry_line(
            "2026-10-01 01:00:00",
            "ERROR",
            &format!("{header}#0 /srv/app/vendor/Handler.php(48): handle()\n\"}}"),
        );
        let current = entry_line(
            "2026-10-01 02:00:00",
            "ERROR",
            &format!("{header}#0 /srv/app/vendor/ShareErrorsFromSession.php(48): handle()\n\"}}"),
        );
        let mut snapshot = snapshot_of(&current, true);
        snapshot.baseline_enabled = true;
        snapshot.channels[0].baseline.available = true;
        snapshot.baseline_entries = parse_entries(baseline.as_bytes(), 0, 0, "laravel.log");
        let groups = group(&snapshot, Level::Error, None);
        let g = &groups.groups[0];
        assert_eq!(g.class, "Error");
        assert_eq!(g.badge, Badge::Seen);
        assert!(g.variants.iter().all(|v| v.badge == Badge::Seen));
        assert_eq!(g.id, "gqmkp");
    }

    #[test]
    fn errors_without_exception_or_file_group_by_message() {
        let text = [
            entry_line(
                "2026-11-08 12:00:00",
                "ERROR",
                "checkout total mismatch {\"order\":1}",
            ),
            entry_line(
                "2026-11-08 12:00:01",
                "ERROR",
                "webhook signature rejected {\"source\":\"x\"}",
            ),
            entry_line(
                "2026-11-08 12:00:02",
                "ERROR",
                "checkout total mismatch {\"order\":2}",
            ),
        ]
        .concat();
        let baseline = entry_line(
            "2026-11-08 11:00:00",
            "ERROR",
            "webhook signature rejected {\"source\":\"y\"}",
        );
        let mut snapshot = snapshot_of(&text, true);
        snapshot.baseline_enabled = true;
        snapshot.channels[0].baseline.available = true;
        snapshot.baseline_entries = parse_entries(baseline.as_bytes(), 0, 0, "laravel.log");
        let groups = group(&snapshot, Level::Error, None);
        let summary: Vec<_> = groups
            .groups
            .iter()
            .map(|g| (g.class.as_str(), g.message.as_str(), g.count, g.badge))
            .collect();
        assert_eq!(
            summary,
            [
                ("Error", "checkout total mismatch", 2, Badge::New),
                ("Error", "webhook signature rejected", 1, Badge::Seen),
            ]
        );
        assert_ne!(groups.groups[0].id, groups.groups[1].id);
    }

    #[test]
    fn since_accepts_durations_and_local_times() {
        assert_eq!(Since::parse("30m"), Ok(Since::Ago(1800)));
        assert_eq!(Since::parse("6h"), Ok(Since::Ago(21_600)));
        assert_eq!(Since::parse("7d"), Ok(Since::Ago(604_800)));
        assert_eq!(
            Since::parse("2026-10-01"),
            Ok(Since::At("2026-10-01".into()))
        );
        assert_eq!(
            Since::parse("2026-10-01 14:00"),
            Ok(Since::At("2026-10-01 14:00".into()))
        );
        for valid in ["2000-02-29", "2096-02-29 14:00", "2100-02-28"] {
            assert_eq!(Since::parse(valid), Ok(Since::At(valid.into())));
        }
        for bad in [
            "",
            "0h",
            "6",
            "h",
            "6x",
            "31d",
            "2026-13-01",
            "0000-01-01",
            "2099-11-31",
            "2099-02-29",
            "2100-02-29 14:00",
            "2026-10-01 24:00",
            "2026-10-01T14:00",
            "2026-10-01 14:00:00",
            "yesterday",
            "1;rm -rf /",
            "éééééééé",
            "123456789é12345",
        ] {
            assert!(Since::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn level_is_a_threshold_in_any_case() {
        assert_eq!(Level::parse("WARNING"), Some(Level::Warning));
        assert_eq!(Level::parse("Error"), Some(Level::Error));
        assert_eq!(Level::parse("fatal"), None);
    }

    #[test]
    fn budget_is_shared_fairly_with_a_floor() {
        const M: u64 = 1024 * 1024;
        let cases: [(&[u64], &[u64]); 5] = [
            (&[100 * M], &[4 * M]),
            (&[M, 2 * M, 3 * M], &[M, 2 * M, 3 * M]),
            (&[8 * M; 6], &[2 * M; 6]),
            (
                &[100, 8 * M, 8 * M, 8 * M, 8 * M, 8 * M],
                &[
                    100,
                    (12 * M - 100) / 5,
                    (12 * M - 100) / 5,
                    (12 * M - 100) / 5,
                    (12 * M - 100) / 5,
                    (12 * M - 100) / 5,
                ],
            ),
            (&[0, 3 * M], &[0, 3 * M]),
        ];
        for (needs, expected) in cases {
            assert_eq!(
                water_fill(needs, 12 * M, 4 * M, 256 * 1024),
                expected,
                "{needs:?}"
            );
        }
        let twenty = water_fill(&[100 * M; 20], 12 * M, 4 * M, 256 * 1024);
        assert!(twenty.iter().all(|given| *given >= 256 * 1024));
        assert!(twenty.iter().sum::<u64>() <= 12 * M);
        assert_eq!(water_fill(&[100 * M], 100, 100, 256 * 1024), [100]);
    }

    #[test]
    fn reads_start_with_the_newest_file() {
        let today = file("storage/logs/laravel-2026-10-02.log", 3_000);
        let yesterday = file("storage/logs/laravel-2026-10-01.log", 5_000);
        let plan = plan_reads(&[&today, &yesterday], 4_000);
        assert_eq!(
            plan.iter()
                .map(|read| (read.start, read.count))
                .collect::<Vec<_>>(),
            [(0, 3_000), (4_000, 1_000)]
        );
    }

    #[test]
    fn daily_files_are_chosen_by_date_from_the_window_start() {
        let source = ChannelSource {
            name: "laravel".into(),
            path: "storage/logs/laravel".into(),
            daily: true,
        };
        let files = [
            file("storage/logs/laravel-2026-09-30.log", 1),
            file("storage/logs/laravel-2026-10-02.log", 1),
            file("storage/logs/laravel-2026-10-01.log", 1),
            file("storage/logs/laravel-worker.log", 1),
            file("storage/logs/laravel-2026-10-01.log.1", 1),
        ];
        let chosen: Vec<&str> = source
            .select(&files, "2026-10-01")
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        assert_eq!(
            chosen,
            [
                "storage/logs/laravel-2026-10-02.log",
                "storage/logs/laravel-2026-10-01.log"
            ]
        );
    }

    #[test]
    fn mid_file_reads_skip_to_the_first_header_and_keep_traces() {
        let text = format!(
            "tail of an older trace\n{}[stacktrace]\n#0 /srv/app/app/Jobs/Run.php(3): go()\n{}",
            entry_line("2026-10-01 01:00:00", "ERROR", "RuntimeException: boom"),
            entry_line("2026-10-01 01:00:05", "INFO", "done"),
        );
        let entries = parse_entries(text.as_bytes(), 100, 0, "laravel.log");
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].offset,
            100 + "tail of an older trace\n".len() as u64
        );
        assert_eq!(
            entries[0].message(),
            "RuntimeException: boom\n[stacktrace]\n#0 /srv/app/app/Jobs/Run.php(3): go()"
        );
        assert_eq!(entries[0].time.as_deref(), Some("2026-10-01 01:00:00"));
        assert_eq!(entries[1].level, Level::Info);
    }

    #[test]
    fn iso_timestamps_are_normalized() {
        assert_eq!(
            normalize_time("2026-10-01T14:00:00.123456+05:30").as_deref(),
            Some("2026-10-01 14:00:00")
        );
        assert_eq!(normalize_time("yesterday"), None);
        assert_eq!(normalize_time("123456789é123456789"), None);
    }

    #[test]
    fn window_split_is_by_position_so_a_dst_fallback_hour_is_kept() {
        // Clocks fell back at 02:00: 01:45 (first pass) then 01:15 (second pass).
        let text = [
            entry_line("2026-11-01 01:30:00", "ERROR", "OldException: before"),
            entry_line("2026-11-01 01:45:00", "ERROR", "AnchorException: at start"),
            entry_line(
                "2026-11-01 01:15:00",
                "ERROR",
                "LaterException: second pass",
            ),
        ]
        .concat();
        let entries = parse_entries(text.as_bytes(), 0, 0, "laravel.log");
        let kept = in_window(entries, "2026-11-01 01:45:00");
        let messages: Vec<&str> = kept.iter().map(|e| e.message()).collect();
        assert_eq!(
            messages,
            ["AnchorException: at start", "LaterException: second pass"]
        );
    }

    #[test]
    fn groups_use_class_and_app_file_with_stable_ids() {
        let text = [
            entry_line(
                "2026-10-01 01:00:00",
                "ERROR",
                "QueryException: Duplicate entry 7 at /srv/app/app/Http/Controllers/OrderController.php:42",
            ),
            entry_line(
                "2026-10-01 02:00:00",
                "ERROR",
                "QueryException: Duplicate entry 9 at /srv/app/app/Http/Controllers/OrderController.php:42",
            ),
            entry_line(
                "2026-10-01 03:00:00",
                "ERROR",
                "QueryException: Deadlock at /srv/app/app/Http/Controllers/OrderController.php:50",
            ),
            entry_line("2026-10-01 04:00:00", "WARNING", "Slow query 3000ms"),
            entry_line("2026-10-01 05:00:00", "CRITICAL", "RuntimeException: other"),
        ]
        .concat();
        let snapshot = snapshot_of(&text, true);
        let groups = group(&snapshot, Level::Error, None);
        assert_eq!(groups.groups.len(), 2);
        let query = &groups.groups[0];
        assert_eq!(query.class, "QueryException");
        assert_eq!(query.count, 3);
        assert_eq!(query.variants.len(), 2);
        assert_eq!(query.variants[0].count, 2);
        assert_eq!(query.first_seen.as_deref(), Some("2026-10-01 01:00:00"));
        assert_eq!(query.last_seen.as_deref(), Some("2026-10-01 03:00:00"));
        assert_eq!(
            query.message,
            "QueryException: Deadlock at /srv/app/app/Http/Controllers/OrderController.php:50"
        );
        assert_eq!(query.id.len(), ID_LEN);
        assert!(query
            .id
            .chars()
            .all(|c| c.is_ascii_lowercase() && c != 'l' && c != 'o'));
        assert_eq!(
            query.id,
            group_id(
                "QueryException",
                Some("/srv/app/app/Http/Controllers/OrderController.php"),
                None
            )[..ID_LEN]
        );

        let warnings = group(&snapshot, Level::Warning, None);
        assert_eq!(warnings.groups.len(), 3);
        let grep = group(&snapshot, Level::Error, Some("DEADLOCK"));
        assert_eq!(grep.groups[0].count, 1);
    }

    #[test]
    fn lookup_by_row_prefix_and_case() {
        let mut groups = Groups::default();
        for (class, full) in [
            ("A", "qmkteaaaaaaaaaaa"),
            ("B", "qmktebbbbbbbbbbb"),
            ("C", "xyzabxyzabxyzabx"),
        ] {
            groups.groups.push(Group {
                badge: Badge::Seen,
                id: String::new(),
                full_id: full.into(),
                class: class.into(),
                file: None,
                normalized_message: None,
                count: 1,
                first_seen: None,
                last_seen: None,
                message: String::new(),
                latest: 0,
                first: 0,
                channels: vec![0],
                variants: Vec::new(),
                overflow_variants: 0,
                partial: false,
            });
        }
        assign_ids(&mut groups.groups);
        assert_eq!(groups.groups[0].id, "qmktea");
        assert!(matches!(find(&groups, "2"), Lookup::Found(g) if g.class == "B"));
        assert!(matches!(find(&groups, "4"), Lookup::Missing));
        assert!(matches!(find(&groups, "XYZ"), Lookup::Found(g) if g.class == "C"));
        assert!(matches!(find(&groups, "QMKTEB"), Lookup::Found(g) if g.class == "B"));
        assert!(matches!(find(&groups, "qmkte"), Lookup::Ambiguous(m) if m.len() == 2));
        assert!(matches!(find(&groups, "zz"), Lookup::Missing));
    }

    #[test]
    fn control_characters_are_escaped() {
        assert_eq!(
            escape("a\x1b[31mred\x1b]0;title\x07\u{9b}\tb\nc\x7f"),
            "a\\x1b[31mred\\x1b]0;title\\x07\\x9b\tb\nc\\x7f"
        );
        assert_eq!(
            escape("evil\u{202e}php.txt \u{2066}x\u{2069} \u{200f}\u{061c}"),
            "evil\\u{202e}php.txt \\u{2066}x\\u{2069} \\u{200f}\\u{61c}"
        );
        assert_eq!(escape_field("a\u{202a}b\n"), "a\\u{202a}b\\x0a");
    }

    #[test]
    fn group_storage_is_bounded_and_detail_retains_the_requested_trace() {
        let text: String = (0..550)
            .map(|i| {
                entry_line(
                    "2026-10-01 01:00:00",
                    "ERROR",
                    &format!(
                        "E{i}Exception: {}\n[stacktrace]\n#0 full trace",
                        "x".repeat(4096)
                    ),
                )
            })
            .collect();
        let snapshot = snapshot_of(&text, true);
        let groups = group(&snapshot, Level::Error, None);
        assert_eq!(groups.groups.len(), MAX_GROUPS);
        assert_eq!(groups.overflow, 50);
        let retained: usize = groups
            .groups
            .iter()
            .map(|group| {
                assert!(group.message.len() <= MESSAGE_BYTES);
                assert!(group
                    .variants
                    .iter()
                    .all(|variant| variant.message.len() <= MESSAGE_BYTES));
                group.message.len()
                    + group
                        .variants
                        .iter()
                        .map(|variant| variant.message.len())
                        .sum::<usize>()
            })
            .sum();
        assert!(retained <= MAX_GROUPS * (MAX_VARIANTS + 1) * MESSAGE_BYTES);
    }

    #[test]
    fn oversized_entries_and_invalid_utf8_keep_byte_offsets_and_report_truncation() {
        let first = entry_line(
            "2026-10-01 01:00:00",
            "ERROR",
            &format!("RuntimeException: {}", "x".repeat(ENTRY_BYTES)),
        );
        let snapshot = snapshot_of(&first, true);
        assert!(snapshot.entries[0].text.len() <= ENTRY_BYTES);
        assert!(snapshot.entries[0].truncated);

        let mut bytes = b"invalid \xff\n".to_vec();
        let start = bytes.len() as u64;
        bytes.extend(
            entry_line("2026-10-01 01:00:00", "ERROR", "RuntimeException: boom").as_bytes(),
        );
        assert_eq!(parse_entries(&bytes, 0, 0, "file")[0].offset, start);
    }

    #[test]
    fn compressed_data_cannot_exceed_or_fall_short_of_the_requested_budget() {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, b"hello\n").unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(gz.finish().unwrap());
        let lines = vec!["@data 0".into(), encoded, "@end".into()];
        assert!(parse_read(0, &lines, &[5]).is_err());
        assert!(parse_read(0, &lines, &[7]).is_err());
        assert!(parse_read(0, &lines, &[6]).is_ok());
        let mut changed = lines;
        changed.insert(2, "@changed 0".into());
        assert_eq!(parse_read(0, &changed, &[6]).unwrap(), [None]);
    }

    #[test]
    fn clock_warning_compares_newest_entry_with_file_write_time() {
        let written = file("storage/logs/laravel.log", 1);
        assert_eq!(
            clock_warning("UTC", "2026-10-01 10:05:00", &written, 2_000),
            None
        );
        let warning = clock_warning("UTC", "2026-10-01 15:30:00", &written, 2_000).unwrap();
        assert!(warning.contains("don't look like UTC"), "{warning}");
        let idle = ProbeFile {
            mtime: 0,
            ..written
        };
        assert_eq!(
            clock_warning("UTC", "2026-10-01 15:30:00", &idle, 200_000),
            None
        );
        assert_eq!(naive_seconds("2026-10-01 10:00:00"), Some(1_790_848_800));
    }

    #[test]
    fn probe_and_read_output_is_parsed() {
        let engine = base64::engine::general_purpose::STANDARD;
        let lines: Vec<String> = [
            "Welcome to the server".to_string(),
            "@time 2000 1000 2026-10-01 00:00:00 2026-10-01 +0000 +0000".into(),
            format!(
                "@file 7 120 1990 1 2026-10-01 00:33:10 abc123 {}",
                engine.encode("/srv/app/storage/logs/pay ments.log")
            ),
            engine.encode("storage/logs/pay ments.log"),
            "@end".into(),
        ]
        .into();
        let probe = parse_probe(0, &lines).unwrap();
        assert_eq!(probe.files[0].path, "storage/logs/pay ments.log");
        assert!(probe.files[0].inside);
        assert!(
            matches!(probe.time, ProbeTime::Known(ref t) if t.start_local == "2026-10-01 00:00:00")
        );
        assert!(matches!(
            parse_probe(0, &["@zone-missing".into()]).unwrap().time,
            ProbeTime::ZoneMissing
        ));
        assert!(parse_probe(0, &lines[..4]).is_err());

        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, b"hello\n").unwrap();
        let data = engine.encode(gz.finish().unwrap());
        let read = parse_read(
            0,
            &["@data 0".into(), data, "@changed 1".into(), "@end".into()],
            &[6, 0],
        )
        .unwrap();
        assert_eq!(read[0].as_ref().unwrap().bytes, b"hello\n");
        assert_eq!(read[1], None);
        assert!(parse_read(0, &["@data 0".into(), "".into()], &[6, 0]).is_err());
    }

    #[test]
    fn scripts_quote_paths_and_recheck_before_reading() {
        let quoted = file("storage/logs/it's.log", 50);
        let plan = plan_reads(&[&quoted], 20);
        let script = read_script("/srv/app", &plan, false);
        assert!(script.contains(r"f='storage/logs/it'\''s.log'"));
        assert!(script.contains("skip=30 count=20"));
        let recheck = script.find("[ \"$1\" = 7 ] && [ \"$2\" -ge 50 ]").unwrap();
        assert!(recheck < script.find("echo \"@data").unwrap());
        assert!(script.contains("/proc/$$/fd/3"));
        assert!(script.contains("check_end || ok=0"));
        assert!(script.contains("case \"$rp\" in \"$root\"/*)"));
        assert!(!read_script("/srv/app", &plan, true).contains("case \"$rp\""));

        let source = ChannelSource {
            name: "laravel".into(),
            path: "storage/logs/laravel".into(),
            daily: true,
        };
        let probe = probe_script(
            "/srv/app",
            "Asia/Kolkata",
            &Since::At("2026-10-01 14:00".into()),
            &source,
            false,
            &Default::default(),
        );
        assert!(probe.contains("tz='Asia/Kolkata'"));
        assert!(probe.contains("date -d '2026-10-01 14:00' +%s"));
        assert!(probe.contains("prefix='storage/logs/laravel'"));
        assert!(probe.contains(r#"for f in "$prefix"-*.log"#));
    }

    #[test]
    fn frames_are_numbered_lines_and_vendor_needs_a_whole_path_segment() {
        assert_eq!(
            frame_rest("#12 /x.php(1): f()"),
            Some((12, "/x.php(1): f()"))
        );
        assert_eq!(frame_rest("  #0\t{main}"), Some((0, "{main}")));
        assert_eq!(frame_rest("#x /x.php(1): f()"), None);
        assert_eq!(frame_rest("#3"), None);
        assert!(vendor_frame("#1 /srv/app/vendor/x/Y.php(3): f()"));
        assert!(vendor_frame("#1 C:\\app\\vendor\\x\\Y.php(3): f()"));
        assert!(!vendor_frame("#1 /srv/my-vendor/app/Y.php(3): f()"));
        assert!(!vendor_frame("#1 /srv/vendors/app/Y.php(3): f()"));
        assert!(!vendor_frame("#1 [internal function]: f()"));
        assert!(!vendor_frame("/srv/app/vendor/x/Y.php(3): f()"));
    }

    #[test]
    fn level_counts_cover_the_whole_window_and_mark_lower_bounds() {
        let text = [
            entry_line("2026-10-01 01:00:00", "ERROR", "RuntimeException: a"),
            entry_line("2026-10-01 01:00:01", "ERROR", "RuntimeException: b"),
            entry_line("2026-10-01 01:00:02", "WARNING", "slow"),
            entry_line("2026-10-01 01:00:03", "INFO", "hello"),
            entry_line("2026-10-01 01:00:04", "CRITICAL", "LogicException: c"),
        ]
        .concat();
        let snapshot = snapshot_of(&text, true);
        assert_eq!(
            level_counts(&snapshot),
            [
                (Level::Critical, 1),
                (Level::Error, 2),
                (Level::Warning, 1),
                (Level::Info, 1)
            ]
        );
        // Partly read: every count is a minimum, and the empty state says so.
        let partial = snapshot_of(&text, false);
        assert!(!coverage_incomplete(&snapshot) && coverage_incomplete(&partial));
        // Fully read but partly understood counts the same way.
        let mut unknown = snapshot_of(&text, true);
        unknown.channels[0].format = Format::Partial(80);
        assert!(coverage_incomplete(&unknown));
        let empty = snapshot_of("", true);
        assert!(level_counts(&empty).is_empty());
    }

    #[test]
    fn group_and_variant_counts_are_minimums_when_content_was_not_understood() {
        let text = [
            entry_line("2026-10-01 01:00:00", "ERROR", "RuntimeException: a"),
            entry_line("2026-10-01 01:00:01", "ERROR", "RuntimeException: a"),
        ]
        .concat();
        let mut snapshot = snapshot_of(&text, true);
        let groups = group(&snapshot, Level::Error, None);
        assert!(!groups.groups[0].partial);

        snapshot.channels[0].format = Format::Unrecognized;
        let groups = group(&snapshot, Level::Error, None);
        let group = &groups.groups[0];
        assert!(group.partial);
        assert_eq!(group.count, 2);
    }

    #[test]
    fn read_time_is_rendered_in_the_log_zone() {
        // 2026-10-01 12:00:00 UTC
        let epoch = naive_seconds("2026-10-01 12:00:00").unwrap() as u64;
        assert_eq!(
            local_time(epoch, "+0000").as_deref(),
            Some("2026-10-01 12:00:00")
        );
        assert_eq!(
            local_time(epoch, "+0530").as_deref(),
            Some("2026-10-01 17:30:00")
        );
        assert_eq!(
            local_time(epoch, "-0700").as_deref(),
            Some("2026-10-01 05:00:00")
        );
        assert_eq!(
            local_time(epoch, "+1400").as_deref(),
            Some("2026-10-02 02:00:00")
        );
        assert_eq!(
            local_time(
                naive_seconds("2028-02-29 23:59:59").unwrap() as u64,
                "+0000"
            )
            .as_deref(),
            Some("2028-02-29 23:59:59")
        );
        assert_eq!(
            local_time(0, "+0000").as_deref(),
            Some("1970-01-01 00:00:00")
        );
        for bad in ["", "0530", "+53", "+05:30", "+0a30"] {
            assert_eq!(local_time(epoch, bad), None, "{bad}");
        }
    }

    #[test]
    fn labels_drop_laravel_context_and_extra() {
        // Lines as Laravel 13 wrote them on the test server.
        let cases = [
            (
                "Invoice backfill failed: column totals_cents is missing (shipslip test) {\"exception\":\"[object] (RuntimeException(code: 0): Invoice backfill failed",
                "Invoice backfill failed: column totals_cents is missing (shipslip test)",
            ),
            (
                "Checkout started (shipslip test)  {\"trace_id\":\"2eee31e7-23cc-4849-b166-d0927fd7251f\"}",
                "Checkout started (shipslip test)",
            ),
            (
                "User authenticated. {\"auth_id\":27} {\"url\":\"https://example.com/login\",\"trace_id\":\"e04e1a11\"}",
                "User authenticated.",
            ),
            (
                "Order failed {\"user\":{\"id\":5},\"exception\":\"[object] (LogicException(code: 0): no",
                "Order failed",
            ),
            ("Queue busy [] []", "Queue busy"),
            (
                "SQLSTATE[HY000]: General error: 8 attempt to write a readonly database",
                "SQLSTATE[HY000]: General error: 8 attempt to write a readonly database",
            ),
            ("Payload was {\"broken", "Payload was {\"broken"),
            // Context and extra are the last two values; earlier text is message.
            ("Three objects {} {} {}", "Three objects {}"),
            ("{\"only\":\"json\"}", "{\"only\":\"json\"}"),
            ("", ""),
        ];
        for (line, label) in cases {
            assert_eq!(without_context(line), label, "{line}");
        }
        // Many brace-like words: only the last few candidates are tried.
        let long = format!("{}{}", " {x".repeat(10_000), " {\"a\":1}");
        assert_eq!(without_context(&long), " {x".repeat(10_000).trim_end());
    }

    fn one(message: &str) -> Entry {
        let line = entry_line("2026-10-01 01:00:00", "ERROR", message);
        parse_entries(line.as_bytes(), 0, 0, "laravel.log").remove(0)
    }

    #[test]
    fn request_ids_come_from_top_level_context_or_extra_only() {
        let id =
            |message: &str| request_id(&one(message)).map(|id| (id.key, id.value, id.conflict));
        let uuid = "2eee31e7-23cc-4849-b166-d0927fd7251f";
        // Laravel 13 with Context::add: empty context omitted, extra after two spaces.
        assert_eq!(
            id(&format!(
                "Payment retry failed  {{\"trace_id\":\"{uuid}\"}}"
            )),
            Some(("trace_id", uuid.to_string(), false))
        );
        // Log::withContext puts it in context; context and extra both present.
        assert_eq!(
            id("Saved {\"request_id\":\"req-12345678\"} {\"url\":\"/x\"}"),
            Some(("request_id", "req-12345678".into(), false))
        );
        // request_id is narrower than trace_id, wherever each sits.
        assert_eq!(
            id("Saved {\"trace_id\":\"trace-1234567\"} {\"request_id\":\"req-12345678\"}"),
            Some(("request_id", "req-12345678".into(), false))
        );
        // Context wins when the two disagree, and says so.
        assert_eq!(
            id("Saved {\"request_id\":\"from-context\"} {\"request_id\":\"from-extra1\"}"),
            Some(("request_id", "from-context".into(), true))
        );
        // After a multi-line exception string, at the top level of context.
        let exception = format!(
            "Boom {{\"exception\":\"[object] (RuntimeException(code: 0): Boom at /srv/app/app/X.php:3)\n[stacktrace]\n#0 /srv/app/vendor/a/B.php(1): \\\"request_id\\\":\\\"from-trace-text\\\"\n#1 {{main}}\n\",\"request_id\":\"req-after-trace\"}} {{\"trace_id\":\"{uuid}\"}}"
        );
        assert_eq!(
            id(&exception),
            Some(("request_id", "req-after-trace".into(), false))
        );
        // Not top level, not context, or not a usable value: no ID.
        for message in [
            "Saved {\"payload\":{\"request_id\":\"nested-1234567\"}}",
            "Got {\"request_id\":\"in-message-123\"} from the API {\"x\":1}",
            "Saved {\"session_id\":\"session-1234567\"}",
            "Saved {\"request_id\":\"short\"}",
            "Saved {\"request_id\":\"has space 12345\"}",
            "Saved {\"request_id\":\"bidi\u{202e}12345678\"}",
            "Saved {\"request_id\":12345678}",
            "Saved {\"request_id\":\"a\"",
            "No context at all",
        ] {
            assert_eq!(id(message), None, "{message}");
        }
        assert_eq!(
            id("Ran {\"job_id\":42}"),
            Some(("job_id", "42".into(), false))
        );
        let long = format!("Saved {{\"request_id\":\"{}\"}}", "a".repeat(129));
        assert_eq!(id(&long), None);
        // Trailing JSON over the size cap is not read.
        let big = format!(
            "Saved {{\"request_id\":\"req-12345678\",\"pad\":\"{}\"}}",
            "x".repeat(ID_TAIL_BYTES)
        );
        assert_eq!(id(&big), None);
    }

    #[test]
    fn related_lines_share_key_and_value_in_time_order() {
        let lines = [
            entry_line(
                "2026-10-01 01:00:05",
                "WARNING",
                "Checkout started  {\"trace_id\":\"trace-aaaaaaaa\"}",
            ),
            entry_line(
                "2026-10-01 01:00:01",
                "INFO",
                "Cart loaded  {\"trace_id\":\"trace-aaaaaaaa\"}",
            ),
            entry_line(
                "2026-10-01 01:00:02",
                "INFO",
                "Other request  {\"trace_id\":\"trace-bbbbbbbb\"}",
            ),
            entry_line(
                "2026-10-01 01:00:03",
                "INFO",
                "Same value, other key  {\"job_id\":\"trace-aaaaaaaa\"}",
            ),
            entry_line(
                "2026-10-01 01:00:09",
                "ERROR",
                "Payment failed  {\"trace_id\":\"trace-aaaaaaaa\"}",
            ),
        ]
        .concat();
        let mut snapshot = snapshot_of(&lines, true);
        snapshot.entries[1].time = None;
        let Correlation::Shared(related) = related(&snapshot, 4) else {
            panic!("expected shared lines")
        };
        assert_eq!(related.id.key, "trace_id");
        assert_eq!(related.before, [0]);
        assert_eq!(related.after, [1]); // untimed sorts after timed
        assert!(!related.partial);
    }

    #[test]
    fn related_lines_list_every_match_and_say_when_some_may_be_missing() {
        let text: String = (0..25)
            .map(|second| {
                entry_line(
                    &format!("2026-10-01 01:00:{second:02}"),
                    if second == 12 { "ERROR" } else { "INFO" },
                    &format!("step {second}  {{\"request_id\":\"req-12345678\"}}"),
                )
            })
            .collect();
        let snapshot = snapshot_of(&text, false);
        let Correlation::Shared(related) = related(&snapshot, 12) else {
            panic!("expected shared lines")
        };
        assert_eq!(related.before, (0..12).collect::<Vec<_>>());
        assert_eq!(related.after, (13..25).collect::<Vec<_>>());
        assert!(related.partial);
    }

    #[test]
    fn entries_without_an_id_get_a_tip_or_a_cut_note() {
        let mut snapshot = snapshot_of(
            &[
                entry_line("2026-10-01 01:00:00", "ERROR", "RuntimeException: boom"),
                entry_line("2026-10-01 01:00:01", "WARNING", "slow query"),
            ]
            .concat(),
            true,
        );
        assert_eq!(related(&snapshot, 0), Correlation::NoId);
        snapshot.entries[0].truncated = true;
        assert_eq!(related(&snapshot, 0), Correlation::Cut);
        let alone = snapshot_of(
            &entry_line(
                "2026-10-01 01:00:00",
                "ERROR",
                "Boom  {\"request_id\":\"req-12345678\"}",
            ),
            true,
        );
        let Correlation::Shared(related) = related(&alone, 0) else {
            panic!("expected an ID")
        };
        assert!(related.before.is_empty() && related.after.is_empty());
    }
}

#[cfg(test)]
#[path = "logs/record_tests.rs"]
mod record_tests;
