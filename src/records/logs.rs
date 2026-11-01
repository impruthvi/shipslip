//! Pure records of an already-read snapshot. No remote reads belong here.
use serde::Serialize;

use super::Untrusted;
use crate::logs::{self, Anchor, AnchorFallback, Groups, Level, Snapshot};
use crate::transport::{ConnectFailure, TransportError};

#[derive(Debug, Serialize)]
pub struct LogsSummary {
    pub environment: Untrusted,
    pub window: LogWindow,
    pub groups: Vec<LogGroup>,
    pub overflow_groups: u64,
    pub filter: LogFilter,
    pub channels: Vec<LogChannel>,
    pub unrecognized_samples: Vec<LogSample>,
    pub warnings: Vec<Untrusted>,
    pub bytes_transferred: u64,
    pub text_fields: Vec<LogTextField>,
}

#[derive(Debug, Serialize)]
pub struct LogWindow {
    pub start: Untrusted,
    pub end: Option<Untrusted>,
    pub zone: Untrusted,
    pub clock_changed: bool,
    pub anchor: Anchor,
    pub anchor_fallbacks: Vec<AnchorFallback>,
}

#[derive(Debug, Serialize)]
pub struct LogFilter {
    pub level: &'static str,
    pub text: Option<Untrusted>,
}

#[derive(Debug, Serialize)]
pub struct LogGroup {
    pub id: Untrusted,
    pub full_id: Untrusted,
    pub badge: LogBadge,
    pub exception: Untrusted,
    pub file: Option<Untrusted>,
    pub count: u64,
    pub count_is_lower_bound: bool,
    pub first_seen: Option<Untrusted>,
    pub last_seen: Option<Untrusted>,
    pub message: Untrusted,
    pub channels: Vec<Untrusted>,
    pub variants: Vec<LogVariant>,
    pub overflow_variants: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogBadge {
    New,
    Seen,
    NoBaseline,
}

#[derive(Debug, Serialize)]
pub struct LogVariant {
    pub badge: LogBadge,
    pub message: Untrusted,
    pub file: Option<Untrusted>,
    pub line: Option<u64>,
    pub count: u64,
}

#[derive(Debug, Serialize)]
pub struct LogChannel {
    pub name: Untrusted,
    pub kind: LogChannelKind,
    pub files: Vec<Untrusted>,
    pub omitted_files: usize,
    pub coverage: LogCoverage,
    pub changed_during_read: Vec<Untrusted>,
    pub size: u64,
    pub last_write: Option<Untrusted>,
    pub format: LogFormat,
    pub baseline: LogBaseline,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogChannelKind {
    Single,
    Daily,
    Mixed,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogCoverage {
    Complete,
    Partial { from: Option<Untrusted> },
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogFormat {
    Recognized,
    Partial { percent: u64 },
    Unrecognized,
    Empty,
    NotRead,
}

#[derive(Debug, Serialize)]
pub struct LogBaseline {
    pub available: bool,
    pub coverage: LogCoverage,
    pub to: Option<Untrusted>,
}

/// Size describes the full recorded text, before any MCP clipping.
#[derive(Debug, Serialize)]
pub struct LogSample {
    pub channel: Untrusted,
    pub file: Untrusted,
    pub text: Untrusted,
    pub clipped: bool,
    pub bytes: usize,
}

/// Builds all tracked groups, without the text renderer's row limit.
pub fn logs_summary(
    snapshot: &Snapshot,
    groups: &Groups,
    level: Level,
    text: Option<&str>,
) -> LogsSummary {
    let mut record = LogsSummary {
        environment: snapshot.env.clone().into(),
        window: LogWindow {
            start: snapshot.start.clone().into(),
            end: snapshot.read_at.clone().map(Into::into),
            zone: snapshot.zone.clone().into(),
            clock_changed: snapshot.clock_changed,
            anchor: snapshot.anchor.clone(),
            anchor_fallbacks: snapshot.anchor_fallbacks.clone(),
        },
        groups: groups
            .groups
            .iter()
            .map(|group| group_record(snapshot, group))
            .collect(),
        overflow_groups: groups.overflow,
        filter: LogFilter {
            level: level.name(),
            text: text.map(Into::into),
        },
        channels: snapshot
            .channels
            .iter()
            .map(|channel| LogChannel {
                name: channel.name.clone().into(),
                kind: match channel.kind {
                    logs::ChannelKind::Single => LogChannelKind::Single,
                    logs::ChannelKind::Daily => LogChannelKind::Daily,
                    logs::ChannelKind::Mixed => LogChannelKind::Mixed,
                },
                files: channel.files.iter().cloned().map(Into::into).collect(),
                omitted_files: channel.omitted_files,
                coverage: coverage(channel.complete, &channel.covered_from),
                changed_during_read: channel.changed.iter().cloned().map(Into::into).collect(),
                size: channel.size,
                last_write: channel.last_write.clone().map(Into::into),
                format: match channel.format {
                    logs::Format::Recognized => LogFormat::Recognized,
                    logs::Format::Partial(percent) => LogFormat::Partial { percent },
                    logs::Format::Unrecognized => LogFormat::Unrecognized,
                    logs::Format::Empty => LogFormat::Empty,
                    logs::Format::NotRead => LogFormat::NotRead,
                },
                baseline: LogBaseline {
                    available: channel.baseline.available,
                    coverage: coverage(channel.baseline.complete, &channel.baseline.covered_from),
                    to: channel.baseline.covered_to.clone().map(Into::into),
                },
            })
            .collect(),
        unrecognized_samples: snapshot
            .raw_blocks
            .iter()
            .map(|sample| LogSample {
                channel: snapshot.channels[sample.channel].name.clone().into(),
                file: sample.file.clone().into(),
                text: sample.text.clone().into(),
                clipped: false,
                bytes: sample.text.len(),
            })
            .collect(),
        warnings: snapshot.warnings.iter().cloned().map(Into::into).collect(),
        bytes_transferred: snapshot.bytes_transferred,
        text_fields: Vec::new(),
    };
    record.text_fields =
        text_fields(&serde_json::to_value(&record).expect("JSON-safe logs record"));
    record
}

fn coverage(complete: bool, from: &Option<String>) -> LogCoverage {
    if complete {
        LogCoverage::Complete
    } else {
        LogCoverage::Partial {
            from: from.clone().map(Into::into),
        }
    }
}

fn badge(snapshot: &Snapshot, badge: logs::Badge) -> LogBadge {
    if !snapshot.baseline_enabled {
        return LogBadge::NoBaseline;
    }
    match badge {
        logs::Badge::New => LogBadge::New,
        logs::Badge::Seen => LogBadge::Seen,
        logs::Badge::Unknown => LogBadge::NoBaseline,
    }
}

fn file_line(at: Option<&str>) -> (Option<Untrusted>, Option<u64>) {
    match at
        .and_then(|at| at.rsplit_once(':'))
        .and_then(|(file, line)| line.parse().ok().map(|line| (file, line)))
    {
        Some((file, line)) => (Some(file.into()), Some(line)),
        None => (at.map(Into::into), None),
    }
}

fn group_record(snapshot: &Snapshot, group: &logs::Group) -> LogGroup {
    LogGroup {
        id: group.id.clone().into(),
        full_id: group.full_id.clone().into(),
        badge: badge(snapshot, group.badge),
        exception: group.class.clone().into(),
        file: group.file.clone().map(Into::into),
        count: group.count,
        count_is_lower_bound: group.partial,
        first_seen: group.first_seen.clone().map(Into::into),
        last_seen: group.last_seen.clone().map(Into::into),
        message: group.message.clone().into(),
        channels: group
            .channels
            .iter()
            .map(|index| snapshot.channels[*index].name.clone().into())
            .collect(),
        variants: group
            .variants
            .iter()
            .map(|variant| {
                let (file, line) = file_line(variant.file_line.as_deref());
                LogVariant {
                    badge: badge(snapshot, variant.badge),
                    message: variant.message.clone().into(),
                    file,
                    line,
                    count: variant.count,
                }
            })
            .collect(),
        overflow_variants: group.overflow_variants,
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogsQueryError {
    InvalidInput {
        message: Untrusted,
    },
    Unreadable {
        message: Untrusted,
    },
    Missing,
    Ambiguous {
        ids: Vec<Untrusted>,
    },
    SshRefused {
        reason: SshRefusal,
        message: Untrusted,
    },
    ConnectionLost {
        message: Untrusted,
    },
    LogPathNotTrusted {
        path: Untrusted,
        environment: Untrusted,
    },
    ZoneMissing {
        zone: Untrusted,
    },
    ServerReadFailed {
        message: Untrusted,
    },
    NotApproved {
        environment: Untrusted,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SshRefusal {
    HostKeyUnknown,
    HostKeyChanged,
    KeyNotLoaded,
    Other,
}

impl LogsQueryError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::InvalidInput {
            message: Untrusted::new(message),
        }
    }
    pub fn exit_code(&self) -> u8 {
        if matches!(self, Self::InvalidInput { .. } | Self::Ambiguous { .. }) {
            2
        } else {
            1
        }
    }
}

impl From<TransportError> for LogsQueryError {
    fn from(error: TransportError) -> Self {
        match error {
            TransportError::ConnectionLost(message) => Self::ConnectionLost {
                message: message.into(),
            },
            TransportError::Connect(message) => Self::SshRefused {
                reason: match ConnectFailure::of(&message) {
                    ConnectFailure::HostKeyUnknown => SshRefusal::HostKeyUnknown,
                    ConnectFailure::HostKeyChanged => SshRefusal::HostKeyChanged,
                    ConnectFailure::KeyNotLoaded => SshRefusal::KeyNotLoaded,
                    ConnectFailure::Other => SshRefusal::Other,
                },
                message: message.into(),
            },
        }
    }
}

impl From<logs::LogsError> for LogsQueryError {
    fn from(error: logs::LogsError) -> Self {
        match error {
            logs::LogsError::Transport(error) => error.into(),
            logs::LogsError::ZoneMissing(zone) => Self::ZoneMissing { zone: zone.into() },
            logs::LogsError::Outside { path, env } => Self::LogPathNotTrusted {
                path: path.into(),
                environment: env.into(),
            },
            error @ (logs::LogsError::BadSince(_) | logs::LogsError::TooOld) => {
                Self::invalid(error.to_string())
            }
            logs::LogsError::Unavailable(message) => Self::ServerReadFailed {
                message: message.into(),
            },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct LogsDetail {
    #[serde(flatten)]
    pub snapshot: LogsSummary,
    pub group: LogGroup,
    pub latest: LatestEntry,
    pub same_request: SameRequest,
}

#[derive(Debug, Serialize)]
pub struct LatestEntry {
    #[serde(flatten)]
    pub entry: LogEntry,
    pub frames: Vec<LogFrame>,
}

#[derive(Debug, Serialize)]
pub struct LogEntry {
    pub time: Option<Untrusted>,
    pub level: &'static str,
    pub channel: Untrusted,
    pub file: Untrusted,
    pub text: Untrusted,
    pub truncated: bool,
    pub clipped: bool,
    pub bytes: usize,
}

#[derive(Debug, Serialize)]
pub struct LogFrame {
    pub index: u64,
    pub file: Option<Untrusted>,
    pub line: Option<u64>,
    pub call: Untrusted,
    pub vendor: bool,
    pub chain: usize,
}

/// Same-request entries are correlation, never evidence of cause.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SameRequest {
    Shared {
        key: &'static str,
        value: Untrusted,
        conflict: bool,
        before: Vec<LogEntry>,
        after: Vec<LogEntry>,
        more: u64,
        may_be_missing: bool,
    },
    NoId,
    Cut,
}

pub fn logs_detail(
    snapshot: &Snapshot,
    group: &logs::Group,
    level: Level,
    text: Option<&str>,
) -> LogsDetail {
    let same_request = match logs::related_with_limit(snapshot, group.latest, usize::MAX) {
        logs::Correlation::Shared(related) => SameRequest::Shared {
            key: related.id.key,
            value: related.id.value.into(),
            conflict: related.id.conflict,
            before: related
                .before
                .iter()
                .map(|i| entry_record(snapshot, *i))
                .collect(),
            after: related
                .after
                .iter()
                .map(|i| entry_record(snapshot, *i))
                .collect(),
            more: related.more,
            may_be_missing: related.partial,
        },
        logs::Correlation::NoId => SameRequest::NoId,
        logs::Correlation::Cut => SameRequest::Cut,
    };
    let mut record = LogsDetail {
        snapshot: logs_summary(snapshot, &Groups::default(), level, text),
        group: group_record(snapshot, group),
        latest: LatestEntry {
            entry: entry_record(snapshot, group.latest),
            frames: frames(&snapshot.entries[group.latest].text),
        },
        same_request,
    };
    record.snapshot.text_fields =
        text_fields(&serde_json::to_value(&record).expect("JSON-safe detail record"));
    record
}

fn entry_record(snapshot: &Snapshot, index: usize) -> LogEntry {
    let entry = &snapshot.entries[index];
    LogEntry {
        time: entry.time.clone().map(Into::into),
        level: entry.level.name(),
        channel: snapshot.channels[entry.channel].name.clone().into(),
        file: entry.file.clone().into(),
        text: entry.text.clone().into(),
        truncated: entry.truncated,
        clipped: false,
        bytes: entry.text.len(),
    }
}

fn frames(text: &str) -> Vec<LogFrame> {
    let mut chain = 0;
    let mut frames = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with("[previous exception]") {
            chain += 1;
        }
        let Some((index, rest)) = logs::frame_rest(line) else {
            continue;
        };
        let (file, line_number, call) = if let Some((location, call)) = rest.split_once("): ") {
            let Some((file, line_number)) = location.rsplit_once('(') else {
                continue;
            };
            let Ok(line_number) = line_number.parse() else {
                continue;
            };
            (Some(file.into()), Some(line_number), call)
        } else if rest == "{main}" || rest.starts_with("[internal function]: ") {
            (
                None,
                None,
                rest.strip_prefix("[internal function]: ").unwrap_or(rest),
            )
        } else {
            continue;
        };
        frames.push(LogFrame {
            index,
            file,
            line: line_number,
            call: call.into(),
            vendor: logs::vendor_frame(line),
            chain,
        });
    }
    frames
}

/// Per-field byte counts use JSON pointers so scalar server text keeps the
/// same JSON shape. Entry and sample text also carry adjacent size fields.
#[derive(Debug, Serialize)]
pub struct LogTextField {
    pub path: String,
    pub clipped: bool,
    pub bytes: usize,
}

fn text_fields(value: &serde_json::Value) -> Vec<LogTextField> {
    fn visit(value: &serde_json::Value, path: &str, fields: &mut Vec<LogTextField>) {
        match value {
            serde_json::Value::String(text) => {
                let key = path.rsplit('/').next().unwrap_or("");
                let fixed =
                    matches!(key, "kind" | "badge" | "level" | "key") || path == "/error/reason";
                if !fixed {
                    fields.push(LogTextField {
                        path: path.into(),
                        clipped: false,
                        bytes: text.len(),
                    });
                }
            }
            serde_json::Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    visit(item, &format!("{path}/{i}"), fields);
                }
            }
            serde_json::Value::Object(object) => {
                for (key, item) in object {
                    if key != "text_fields" {
                        visit(item, &format!("{path}/{key}"), fields);
                    }
                }
            }
            _ => {}
        }
    }
    let mut fields = Vec::new();
    visit(value, "", &mut fields);
    fields
}

/// Applies the MCP budget after frames have been parsed from full text.
/// The CLI builders populate the same metadata without clipping anything.
pub fn limit_log_text(record: &mut serde_json::Value, budget: usize) {
    if let Some(shared) = record
        .get_mut("same_request")
        .filter(|shared| shared["kind"] == "shared")
    {
        let mut omitted = 0;
        for side in ["before", "after"] {
            if let Some(entries) = shared[side].as_array_mut() {
                if entries.len() > 5 {
                    let excess = entries.len() - 5;
                    omitted += excess as u64;
                    if side == "before" {
                        entries.drain(..excess);
                    } else {
                        entries.truncate(5);
                    }
                }
            }
        }
        shared["more"] = serde_json::json!(shared["more"].as_u64().unwrap_or(0) + omitted);
    }
    let mut fields = text_fields(record);
    let mut clipped_text = Vec::new();
    for field in &mut fields {
        if field.bytes <= budget {
            continue;
        }
        if let Some(value) = record.pointer_mut(&field.path) {
            if let Some(text) = value.as_str() {
                let marker = "\n… [clipped] …\n";
                let kept = budget.saturating_sub(marker.len());
                let mut head = kept / 2;
                while head > 0 && !text.is_char_boundary(head) {
                    head -= 1;
                }
                let mut tail = text.len() - (kept - kept / 2);
                while tail < text.len() && !text.is_char_boundary(tail) {
                    tail += 1;
                }
                *value = serde_json::json!(format!("{}{marker}{}", &text[..head], &text[tail..]));
                field.clipped = true;
                if let Some(parent) = field.path.strip_suffix("/text") {
                    clipped_text.push(parent.to_string());
                }
            }
        }
    }
    for path in clipped_text {
        if let Some(parent) = record.pointer_mut(&path) {
            parent["clipped"] = serde_json::json!(true);
        }
    }
    record["text_fields"] = serde_json::to_value(fields).expect("JSON-safe text metadata");
}

/// Local record tools keep their existing shape unless a field needs clipping.
pub fn limit_record_text(record: &mut serde_json::Value, budget: usize) {
    if text_fields(record).iter().any(|field| field.bytes > budget) {
        limit_log_text(record, budget);
    }
}
