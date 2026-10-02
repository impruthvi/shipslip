//! Bounded Laravel log observation and a local HTTP smoke check.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine as _;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};

use crate::event::DeployEvent;
use crate::runner::run_collect;
use crate::script::shell_quote;
use crate::transport::Transport;
use crate::DeployTarget;

const BASELINE_BYTES: usize = 2 * 1024 * 1024;
const POLL_BYTES: usize = 256 * 1024;
pub(crate) const ENTRY_BYTES: usize = 256 * 1024;
const MAX_SIGNATURES: usize = 10_000;
pub(crate) const MAX_GROUPS: usize = 500;
pub(crate) const MAX_VARIANTS: usize = 50;
const POLL_EVERY: Duration = Duration::from_millis(500);
const MAX_HISTORY_BYTES: u64 = 16 * 1024 * 1024;
const MAX_HISTORY: usize = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogPhase {
    During,
    After,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WatchStatus {
    Complete,
    Partial,
    Unavailable,
    Cancelled,
    NotRun,
    NoLogSeen,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorVariant {
    pub signature: String,
    pub level: String,
    pub message: String,
    pub display_file_line: Option<String>,
    pub count: u64,
    pub phase: LogPhase,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorGroup {
    pub exception: String,
    pub file: Option<String>,
    pub count: u64,
    pub variants: Vec<ErrorVariant>,
    pub overflow_variants: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchResult {
    pub status: WatchStatus,
    pub log_path: Option<String>,
    pub duration_ms: u64,
    pub baseline_signatures: usize,
    pub history_signatures: usize,
    pub observed_lines: u64,
    pub parsed_lines: u64,
    /// Always 0; kept so receipts keep their shape.
    pub dropped_view_lines: u64,
    pub truncated_entries: u64,
    pub new_errors: Vec<ErrorGroup>,
    pub overflow_groups: u64,
    pub overflow_signatures: u64,
    pub warnings: Vec<String>,
}

impl WatchResult {
    pub fn not_run() -> Self {
        Self {
            status: WatchStatus::NotRun,
            log_path: None,
            duration_ms: 0,
            baseline_signatures: 0,
            history_signatures: 0,
            observed_lines: 0,
            parsed_lines: 0,
            dropped_view_lines: 0,
            truncated_entries: 0,
            new_errors: Vec::new(),
            overflow_groups: 0,
            overflow_signatures: 0,
            warnings: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SmokeResult {
    NotConfigured,
    Skipped,
    Passed { status: u16, latency_ms: u64 },
    Failed { status: Option<u16>, reason: String },
}

#[derive(Clone, Copy)]
enum Mode {
    Idle,
    During,
    After(Instant),
    Stop(WatchStatus),
}

pub(crate) struct LogObserver {
    control: watch::Sender<Mode>,
    task: tokio::task::JoinHandle<WatchResult>,
}

impl LogObserver {
    pub(crate) async fn start<T: Transport>(
        transport: Arc<T>,
        target: &DeployTarget,
        events: mpsc::UnboundedSender<DeployEvent>,
        history_path: Option<PathBuf>,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        let spec = LogSpec::from_target(target);
        let (history, history_error) = match history_path.as_deref().map(read_history) {
            Some(Ok(history)) => (history, None),
            Some(Err(error)) => (Vec::new(), Some(error)),
            None => (Vec::new(), None),
        };
        let mut state = LogState::new(history);
        if let Some(error) = history_error {
            state.partial = true;
            state.warn(format!("could not read signature history: {error}"));
        }
        match tokio::time::timeout(
            Duration::from_secs(10),
            fetch(&*transport, &spec, &state.cursor, true),
        )
        .await
        .unwrap_or_else(|_| Fetch::Unavailable("log baseline timed out".into()))
        {
            Fetch::File {
                path,
                inode,
                size,
                bytes,
                ..
            } => {
                state.saw_file = true;
                state.result.log_path = Some(path.clone());
                state.bytes(&bytes, LogPhase::During, true, &events);
                state.flush(true, &events);
                state.partial_line.clear();
                state.cursor = Cursor {
                    path,
                    inode,
                    offset: size,
                };
            }
            Fetch::Missing => {}
            Fetch::Unavailable(reason) => {
                state.partial = true;
                state.unavailable = true;
                state.warn(reason);
            }
        }
        let (control, receiver) = watch::channel(Mode::Idle);
        let task = tokio::spawn(watch_loop(
            transport,
            spec,
            events,
            history_path,
            state,
            receiver,
            cancel,
        ));
        Self { control, task }
    }

    pub(crate) fn begin(&self) {
        let _ = self.control.send(Mode::During);
    }

    pub(crate) fn abort(self) {
        self.task.abort();
    }

    pub(crate) async fn finish(self, status: WatchStatus, post_window: Duration) -> WatchResult {
        if status == WatchStatus::Complete {
            let _ = self.control.send(Mode::After(Instant::now() + post_window));
        } else {
            let _ = self.control.send(Mode::Stop(status));
        }
        self.task.await.unwrap_or_else(|error| WatchResult {
            status: WatchStatus::Partial,
            warnings: vec![format!("log watcher stopped unexpectedly: {error}")],
            ..WatchResult::not_run()
        })
    }
}

struct LogSpec {
    app_path: String,
    path: String,
    daily: bool,
}

impl LogSpec {
    fn from_target(target: &DeployTarget) -> Self {
        Self {
            app_path: target.path.clone(),
            path: target.log_path().into(),
            daily: target.log_daily,
        }
    }
}

#[derive(Default)]
struct Cursor {
    path: String,
    inode: String,
    offset: u64,
}

enum Fetch {
    File {
        path: String,
        inode: String,
        size: u64,
        start: u64,
        bytes: Vec<u8>,
        rotated: bool,
    },
    Missing,
    Unavailable(String),
}

fn fetch_script(spec: &LogSpec, cursor: &Cursor, baseline: bool) -> String {
    let limit = if baseline { BASELINE_BYTES } else { POLL_BYTES };
    let choose = if spec.daily {
        format!(
            "prefix={}\nnewest=-1\nf=\nfor candidate in \"$prefix\"-*.log; do\n  [ -f \"$candidate\" ] || continue\n  changed=$(stat -c %Y \"$candidate\" 2>/dev/null) || continue\n  if [ \"$changed\" -ge \"$newest\" ]; then newest=$changed; f=$candidate; fi\ndone\n",
            shell_quote(&spec.path)
        )
    } else {
        format!("f={}\n", shell_quote(&spec.path))
    };
    format!(
        r#"cd {app} || exit 1
{choose}
if [ -z "$f" ] || [ ! -e "$f" ]; then
  d=$(dirname {path})
  if [ -r "$d" ]; then echo @missing; else echo @unavailable; fi
  exit 0
fi
if [ ! -r "$f" ]; then echo @unavailable; exit 0; fi
meta=$(stat -c '%i %s' "$f") || {{ echo @unavailable; exit 0; }}
set -- $meta
inode=$1
size=$2
start={offset}
rotated=0
if [ "$f" != {cursor_path} ] || [ "$inode" != {inode} ] || [ "$size" -lt {offset} ]; then start=0; rotated=1; fi
if [ {baseline} -eq 1 ]; then start=$((size > {baseline_bytes} ? size - {baseline_bytes} : 0)); rotated=0; fi
count=$((size - start))
[ "$count" -gt {limit} ] && count={limit}
printf '@file %s %s %s %s %s\n' "$inode" "$size" "$start" "$count" "$rotated"
printf '%s' "$f" | base64 -w0; echo
if [ "$count" -gt 0 ]; then tail -c +$((start + 1)) "$f" | head -c "$count" | base64 -w0; fi
echo
"#,
        app = shell_quote(&spec.app_path),
        choose = choose,
        path = shell_quote(&spec.path),
        offset = cursor.offset,
        cursor_path = shell_quote(&cursor.path),
        inode = shell_quote(&cursor.inode),
        baseline = u8::from(baseline),
        baseline_bytes = BASELINE_BYTES,
        limit = limit,
    )
}

fn parse_fetch(code: i32, lines: &[String]) -> Fetch {
    if code != 0 {
        return Fetch::Unavailable(format!("log read exited with {code}"));
    }
    let marker = lines.iter().position(|line| {
        line == "@missing" || line == "@unavailable" || line.starts_with("@file ")
    });
    let Some(marker) = marker else {
        return Fetch::Unavailable("log response had no status marker".into());
    };
    let lines = &lines[marker..];
    match lines.first().map(String::as_str) {
        Some("@missing") => Fetch::Missing,
        Some("@unavailable") => Fetch::Unavailable("log file or directory is not readable".into()),
        Some(first) if first.starts_with("@file ") && lines.len() >= 3 => {
            let fields: Vec<_> = first.split_ascii_whitespace().collect();
            if fields.len() != 6 {
                return Fetch::Unavailable("unexpected log metadata".into());
            }
            let parsed = (
                fields[2].parse::<u64>(),
                fields[3].parse::<u64>(),
                fields[4].parse::<usize>(),
            );
            let (Ok(size), Ok(start), Ok(count)) = parsed else {
                return Fetch::Unavailable("invalid log metadata".into());
            };
            let engine = base64::engine::general_purpose::STANDARD;
            let (Ok(path), Ok(bytes)) = (engine.decode(&lines[1]), engine.decode(&lines[2])) else {
                return Fetch::Unavailable("invalid log data".into());
            };
            if bytes.len() != count {
                return Fetch::Unavailable("incomplete log read".into());
            }
            Fetch::File {
                path: String::from_utf8_lossy(&path).into_owned(),
                inode: fields[1].to_string(),
                size,
                start,
                bytes,
                rotated: fields[5] == "1",
            }
        }
        _ => Fetch::Unavailable("unexpected log response".into()),
    }
}

async fn fetch<T: Transport>(
    transport: &T,
    spec: &LogSpec,
    cursor: &Cursor,
    baseline: bool,
) -> Fetch {
    let (result, lines) = run_collect(transport, &fetch_script(spec, cursor, baseline)).await;
    match result {
        Ok(code) => parse_fetch(code, &lines),
        Err(error) => Fetch::Unavailable(error.to_string()),
    }
}

struct LogState {
    result: WatchResult,
    known: HashSet<String>,
    /// Signature history, most recently seen first.
    history: Vec<String>,
    baseline: HashSet<String>,
    seen: HashSet<String>,
    group_index: HashMap<(String, Option<String>), usize>,
    overflow_group_keys: HashSet<(String, Option<String>)>,
    partial_line: Vec<u8>,
    dropping_line: bool,
    current_entry: Option<String>,
    current_header: Option<Header>,
    current_phase: LogPhase,
    last_entry_update: Instant,
    cursor: Cursor,
    saw_file: bool,
    partial: bool,
    unavailable: bool,
    /// The last read stopped before the end of the file.
    behind: bool,
    started: Instant,
}

impl LogState {
    fn warn(&mut self, warning: impl Into<String>) {
        let warning = warning.into();
        if self.result.warnings.len() < 20 && !self.result.warnings.contains(&warning) {
            self.result.warnings.push(warning);
        }
    }

    fn new(history: Vec<String>) -> Self {
        let mut result = WatchResult::not_run();
        let known: HashSet<String> = history.iter().cloned().collect();
        result.history_signatures = known.len();
        Self {
            result,
            known,
            history,
            baseline: HashSet::new(),
            seen: HashSet::new(),
            group_index: HashMap::new(),
            overflow_group_keys: HashSet::new(),
            partial_line: Vec::new(),
            dropping_line: false,
            current_entry: None,
            current_header: None,
            current_phase: LogPhase::During,
            last_entry_update: Instant::now(),
            cursor: Cursor::default(),
            saw_file: false,
            partial: false,
            unavailable: false,
            behind: false,
            started: Instant::now(),
        }
    }

    fn bytes(
        &mut self,
        bytes: &[u8],
        phase: LogPhase,
        baseline: bool,
        events: &mpsc::UnboundedSender<DeployEvent>,
    ) {
        for byte in bytes {
            if *byte == b'\n' {
                let line = String::from_utf8_lossy(&self.partial_line).into_owned();
                self.line(line, phase, baseline, events);
                self.partial_line.clear();
                self.dropping_line = false;
            } else if !self.dropping_line {
                if self.partial_line.len() < ENTRY_BYTES {
                    self.partial_line.push(*byte);
                } else {
                    self.result.truncated_entries += 1;
                    self.partial_line.extend_from_slice(b" [truncated]");
                    self.dropping_line = true;
                }
            }
        }
    }

    fn line(
        &mut self,
        line: String,
        phase: LogPhase,
        baseline: bool,
        events: &mpsc::UnboundedSender<DeployEvent>,
    ) {
        if !baseline {
            self.result.observed_lines += 1;
        }
        if let Some(header) = parse_header(&line) {
            self.flush(baseline, events);
            self.current_entry = Some(header.message.clone());
            self.current_header = Some(header);
            self.current_phase = phase;
            self.last_entry_update = Instant::now();
            if !baseline {
                self.result.parsed_lines += 1;
            }
        } else if let Some(entry) = &mut self.current_entry {
            if !baseline {
                self.result.parsed_lines += 1;
            }
            if entry.len() + line.len() < ENTRY_BYTES {
                entry.push('\n');
                entry.push_str(&line);
                self.last_entry_update = Instant::now();
            } else {
                self.result.truncated_entries += 1;
            }
        }
    }

    fn flush(&mut self, baseline: bool, events: &mpsc::UnboundedSender<DeployEvent>) {
        let (Some(header), Some(entry)) = (self.current_header.take(), self.current_entry.take())
        else {
            return;
        };
        if !matches!(
            header.level.as_str(),
            "ERROR" | "CRITICAL" | "ALERT" | "EMERGENCY"
        ) {
            return;
        }
        let (signature, class, file, display) = signature(&header.level, &entry);
        if baseline {
            self.baseline.insert(signature.clone());
            self.known.insert(signature);
            return;
        }
        if self.seen.len() >= MAX_SIGNATURES && !self.seen.contains(&signature) {
            self.result.overflow_signatures += 1;
            return;
        }
        let new_to_run = self.seen.insert(signature.clone());
        if self.known.contains(&signature) {
            return;
        }
        let key = (class.clone(), file.clone());
        let group = if let Some(&index) = self.group_index.get(&key) {
            &mut self.result.new_errors[index]
        } else if self.result.new_errors.len() < MAX_GROUPS {
            let index = self.result.new_errors.len();
            self.group_index.insert(key, index);
            self.result.new_errors.push(ErrorGroup {
                exception: class,
                file,
                count: 0,
                variants: Vec::new(),
                overflow_variants: 0,
            });
            &mut self.result.new_errors[index]
        } else {
            if self.overflow_group_keys.insert(key) {
                self.result.overflow_groups += 1;
            }
            return;
        };
        group.count += 1;
        if let Some(variant) = group
            .variants
            .iter_mut()
            .find(|item| item.signature == signature)
        {
            variant.count += 1;
        } else if group.variants.len() < MAX_VARIANTS {
            let message = clip_utf8(&header.message, 1024);
            group.variants.push(ErrorVariant {
                signature,
                level: header.level,
                message: message.clone(),
                display_file_line: display.clone(),
                count: 1,
                phase: self.current_phase,
            });
            let _ = events.send(DeployEvent::NewLogError {
                phase: self.current_phase,
                message,
                file_line: display,
            });
        } else {
            if new_to_run {
                group.overflow_variants += 1;
            }
        }
    }

    fn done(
        mut self,
        requested: WatchStatus,
        events: &mpsc::UnboundedSender<DeployEvent>,
    ) -> (WatchResult, Vec<String>) {
        if requested == WatchStatus::NotRun {
            return (WatchResult::not_run(), Vec::new());
        }
        self.flush(false, events);
        if self.behind {
            self.partial = true;
            self.warn("Log grew faster than it could be read; later entries were not checked");
        }
        self.result.duration_ms = self.started.elapsed().as_millis() as u64;
        self.result.baseline_signatures = self.baseline.len();
        self.result.status = match requested {
            WatchStatus::NotRun => WatchStatus::NotRun,
            WatchStatus::Cancelled => WatchStatus::Cancelled,
            WatchStatus::Partial => WatchStatus::Partial,
            _ if self.unavailable => WatchStatus::Unavailable,
            _ if !self.saw_file => WatchStatus::NoLogSeen,
            _ if self.partial
                || (self.result.observed_lines >= 20
                    && self.result.parsed_lines * 2 < self.result.observed_lines) =>
            {
                WatchStatus::Partial
            }
            _ => WatchStatus::Complete,
        };
        if self.result.status == WatchStatus::NoLogSeen {
            self.warn("No log file appeared; check LOG_CHANNEL");
        }
        if self.result.status == WatchStatus::Partial
            && self.result.observed_lines >= 20
            && self.result.parsed_lines * 2 < self.result.observed_lines
        {
            self.warn("Log format not recognized; errors may be missed");
        }
        let current = self.seen.into_iter().chain(self.baseline);
        (self.result, recent_first(current, self.history))
    }
}

/// Signatures seen in this run, then older history, without duplicates.
fn recent_first(current: impl IntoIterator<Item = String>, previous: Vec<String>) -> Vec<String> {
    let mut current: Vec<String> = current.into_iter().collect();
    current.sort();
    let mut added = HashSet::new();
    current
        .into_iter()
        .chain(previous)
        .filter(|signature| added.insert(signature.clone()))
        .take(MAX_HISTORY)
        .collect()
}

#[derive(Clone)]
pub(crate) struct Header {
    /// The text between the leading brackets, as Laravel wrote it.
    pub(crate) timestamp: String,
    pub(crate) level: String,
    pub(crate) message: String,
}

pub(crate) fn parse_header(line: &str) -> Option<Header> {
    let (timestamp, rest) = line.strip_prefix('[')?.split_once("] ")?;
    let (level_part, message) = rest.split_once(": ")?;
    let level = level_part.rsplit('.').next()?.to_ascii_uppercase();
    if !matches!(
        level.as_str(),
        "DEBUG" | "INFO" | "NOTICE" | "WARNING" | "ERROR" | "CRITICAL" | "ALERT" | "EMERGENCY"
    ) {
        return None;
    }
    Some(Header {
        timestamp: timestamp.into(),
        level,
        message: message.into(),
    })
}

/// Prefer Laravel's root exception object over mentions in the message or trace.
/// Monolog may turn escaped newlines into literal line breaks, so the context
/// string is not always valid JSON. In that case, read only its object prefix.
fn exception_class(entry: &str) -> String {
    static RE: OnceLock<(Regex, Regex, Regex)> = OnceLock::new();
    let (field_re, object_re, plain_re) = RE.get_or_init(|| {
        let class = r"(\\?[A-Za-z_][A-Za-z0-9_]*(?:\\+[A-Za-z_][A-Za-z0-9_]*)*)";
        (
            Regex::new(r#"(?:^|[,{])\s*"exception"\s*:\s*(")"#).unwrap(),
            Regex::new(&format!(r"^\[object\]\s+\({class}\(code:")).unwrap(),
            Regex::new(&format!(r"^\s*{class}(?::|\(code:|\s+at(?:\s|$))")).unwrap(),
        )
    });
    let canonical = |class: &str| {
        let class = class
            .split('\\')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\\");
        clip_utf8(&class, 256)
    };
    let first_line = entry.lines().next().unwrap_or(entry);
    if let Some(field) = field_re.captures(first_line) {
        let value = &entry[field.get(1).unwrap().start()..];
        let decoded = serde_json::Deserializer::from_str(value)
            .into_iter::<String>()
            .next()
            .and_then(Result::ok);
        let object = decoded
            .as_deref()
            .unwrap_or_else(|| value.strip_prefix('"').unwrap_or(value));
        if let Some(class) = object_re.captures(object) {
            return canonical(&class[1]);
        }
    }
    if let Some(class) = plain_re.captures(first_line) {
        let name = class[1]
            .rsplit('\\')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if name.ends_with("exception") || name.ends_with("error") {
            return canonical(&class[1]);
        }
    }
    "Error".into()
}

fn patterns() -> &'static (Regex, Regex, Regex, Regex, Regex) {
    static RE: OnceLock<(Regex, Regex, Regex, Regex, Regex)> = OnceLock::new();
    RE.get_or_init(|| {
        (
            // `app/` must be a whole path segment: `/srv/my-app/vendor/...` is not an app file.
            Regex::new(
                r"(?:^|[^A-Za-z0-9_.-])((?:/?[A-Za-z0-9_.-]+/)*?app/[A-Za-z0-9_./-]+\.php)(?::(\d+)|\((\d+)\))",
            )
            .unwrap(),
            Regex::new(r"[0-9a-fA-F]{8}-[0-9a-fA-F-]{27,}").unwrap(),
            Regex::new(r"0x[0-9a-fA-F]+|\b\d+\b").unwrap(),
            Regex::new(r"'[^']{65,}'|'[^']*\s[^']*'").unwrap(),
            Regex::new(r#""[^"]{65,}"|"[^"]*\s[^"]*""#).unwrap(),
        )
    })
}

pub(crate) fn signature(
    level: &str,
    entry: &str,
) -> (String, String, Option<String>, Option<String>) {
    let (frame_re, uuid_re, number_re, single_quote_re, double_quote_re) = patterns();
    let class = exception_class(entry);
    let frame = frame_re
        .captures_iter(entry)
        .find(|m| !m[1].contains("vendor/"));
    let file = frame.as_ref().map(|m| clip_utf8(&m[1], 512));
    let display = frame.as_ref().map(|m| {
        let line = m
            .get(2)
            .or_else(|| m.get(3))
            .map_or("", |line| line.as_str());
        format!("{}:{line}", clip_utf8(&m[1], 512))
    });
    let first_line = clip_utf8(entry.lines().next().unwrap_or(entry), 2048);
    let normalized = single_quote_re.replace_all(&first_line, "<quoted>");
    let normalized = double_quote_re.replace_all(&normalized, "<quoted>");
    let normalized = uuid_re.replace_all(&normalized, "<uuid>");
    let normalized = number_re.replace_all(&normalized, "<num>");
    (
        format!(
            "{level}|{class}|{}|{normalized}",
            file.as_deref().unwrap_or("")
        ),
        class,
        file,
        display,
    )
}

pub(crate) fn clip_utf8(value: &str, limit: usize) -> String {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

async fn watch_loop<T: Transport>(
    transport: Arc<T>,
    spec: LogSpec,
    events: mpsc::UnboundedSender<DeployEvent>,
    history_path: Option<PathBuf>,
    mut state: LogState,
    mut control: watch::Receiver<Mode>,
    cancel: Arc<AtomicBool>,
) -> WatchResult {
    let mut after_caught_up = false;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return finalize_watch(
                state,
                WatchStatus::Cancelled,
                &events,
                history_path.as_deref(),
            );
        }
        let mode = *control.borrow();
        if matches!(mode, Mode::Idle) {
            if control.changed().await.is_err() {
                return state.done(WatchStatus::Partial, &events).0;
            }
            continue;
        }
        if let Mode::Stop(status) = mode {
            return finalize_watch(state, status, &events, history_path.as_deref());
        }
        let catch_up = if let Mode::After(deadline) = mode {
            if after_caught_up && Instant::now() >= deadline {
                return finalize_watch(
                    state,
                    WatchStatus::Complete,
                    &events,
                    history_path.as_deref(),
                );
            }
            let needs_catch_up = !after_caught_up;
            after_caught_up = true;
            needs_catch_up
        } else {
            false
        };
        if !catch_up && !state.behind {
            tokio::select! {
                _ = tokio::time::sleep(POLL_EVERY) => {},
                changed = control.changed() => {
                    if changed.is_err() {
                        return state.done(WatchStatus::Partial, &events).0;
                    }
                    continue;
                },
            }
        }
        let phase = if catch_up {
            LogPhase::During
        } else if matches!(*control.borrow(), Mode::After(_)) {
            LogPhase::After
        } else {
            LogPhase::During
        };
        let recovering_baseline = state.unavailable && !state.saw_file;
        match tokio::time::timeout(
            Duration::from_secs(10),
            fetch(&*transport, &spec, &state.cursor, recovering_baseline),
        )
        .await
        .unwrap_or_else(|_| Fetch::Unavailable("log poll timed out".into()))
        {
            Fetch::File {
                path,
                inode,
                size,
                start,
                bytes,
                rotated,
            } => {
                if rotated && state.saw_file {
                    state.partial = true;
                    state.warn("Log rotated or truncated during watch");
                    state.partial_line.clear();
                    state.flush(false, &events);
                }
                state.saw_file = true;
                state.unavailable = false;
                state.behind = !recovering_baseline && start + (bytes.len() as u64) < size;
                state.result.log_path = Some(path.clone());
                state.cursor = Cursor {
                    path,
                    inode,
                    offset: if recovering_baseline {
                        size
                    } else {
                        start + bytes.len() as u64
                    },
                };
                state.bytes(&bytes, phase, recovering_baseline, &events);
                if recovering_baseline {
                    state.flush(true, &events);
                    state.partial_line.clear();
                    state.warn("Log baseline recovered; entries during the gap may be missing");
                } else if bytes.is_empty()
                    && state.current_entry.is_some()
                    && state.last_entry_update.elapsed() >= Duration::from_secs(1)
                {
                    state.flush(false, &events);
                }
            }
            // The log directory was readable, so a file created later is new.
            Fetch::Missing => {
                state.unavailable = false;
                state.behind = false;
            }
            Fetch::Unavailable(reason) => {
                state.partial = true;
                state.unavailable = true;
                state.behind = false;
                state.warn(reason);
                let _ = transport.reconnect().await;
            }
        }
    }
}

fn finalize_watch(
    state: LogState,
    status: WatchStatus,
    events: &mpsc::UnboundedSender<DeployEvent>,
    history_path: Option<&Path>,
) -> WatchResult {
    let (mut result, signatures) = state.done(status, events);
    if status != WatchStatus::NotRun {
        if let Some(path) = history_path {
            if let Err(error) = write_history(path, &signatures) {
                if result.status == WatchStatus::Complete {
                    result.status = WatchStatus::Partial;
                }
                if result.warnings.len() < 20 {
                    result
                        .warnings
                        .push(format!("could not save signature history: {error}"));
                }
            }
        }
    }
    result
}

fn read_history(path: &Path) -> Result<Vec<String>, std::io::Error> {
    match std::fs::File::open(path) {
        Ok(mut file) => {
            use std::io::Read;
            if file.metadata()?.len() > MAX_HISTORY_BYTES {
                return Err(std::io::Error::other("signature history is too large"));
            }
            let mut bytes = Vec::new();
            file.by_ref()
                .take(MAX_HISTORY_BYTES + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_HISTORY_BYTES {
                return Err(std::io::Error::other("signature history is too large"));
            }
            let values: Vec<String> = serde_json::from_slice(&bytes)?;
            Ok(values.into_iter().take(MAX_HISTORY).collect())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn write_history(path: &Path, signatures: &[String]) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for directory in parent.ancestors().take(3) {
                std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
            }
        }
    }
    crate::private_file::write_atomic(path, &serde_json::to_vec(signatures)?)
}

pub async fn smoke_check(url: Option<&str>) -> SmokeResult {
    let Some(url) = url else {
        return SmokeResult::NotConfigured;
    };
    let command = tokio::process::Command::new("curl")
        .args([
            // Must come first: ignore ~/.curlrc, which could turn off TLS checks.
            "--disable",
            "--silent",
            "--show-error",
            "--location",
            "--max-redirs",
            "3",
            "--max-time",
            "10",
            "--proto",
            "=http,https",
            "--proto-redir",
            "=http,https",
            "--output",
            "/dev/null",
            "--write-out",
            "shipslip:%{http_code} %{time_total}",
            "--",
            url,
        ])
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(Duration::from_secs(11), command).await {
        Ok(Ok(output)) => {
            let (status, latency_ms) = parse_write_out(&String::from_utf8_lossy(&output.stdout));
            if output.status.success() && status.is_some_and(|code| (200..300).contains(&code)) {
                SmokeResult::Passed {
                    status: status.unwrap(),
                    latency_ms,
                }
            } else {
                let reason = if output.status.success() {
                    status.map_or_else(|| "no HTTP response".into(), |code| format!("HTTP {code}"))
                } else {
                    String::from_utf8_lossy(&output.stderr).trim().to_string()
                };
                SmokeResult::Failed { status, reason }
            }
        }
        Ok(Err(error)) => SmokeResult::Failed {
            status: None,
            reason: error.to_string(),
        },
        Err(_) => SmokeResult::Failed {
            status: None,
            reason: "timed out after 10 seconds".into(),
        },
    }
}

/// Parses curl's `shipslip:%{http_code} %{time_total}` output.
fn parse_write_out(text: &str) -> (Option<u16>, u64) {
    let parts: Vec<_> = text
        .trim()
        .strip_prefix("shipslip:")
        .unwrap_or("")
        .split_whitespace()
        .collect();
    // curl reports 000 when no HTTP response arrived.
    let status = parts
        .first()
        .and_then(|code| code.parse::<u16>().ok())
        .filter(|code| *code != 0);
    let latency_ms = parts
        .get(1)
        .and_then(|seconds| seconds.parse::<f64>().ok())
        .map(|seconds| (seconds * 1000.0) as u64)
        .unwrap_or(0);
    (status, latency_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TransportError;
    use std::sync::Mutex;

    const OLD_ERROR: &[u8] = b"[2026-09-01 00:00:00] production.ERROR: OldException: old failure\n";
    const NEW_ERROR: &[u8] = b"[2026-09-01 00:00:00] production.ERROR: NewException: new failure\n";

    #[derive(Clone)]
    enum Read {
        Lost,
        Missing,
        File {
            size: usize,
            start: usize,
            bytes: &'static [u8],
        },
    }

    /// Answers log reads in order, repeating the last answer.
    struct LogTransport {
        reads: Mutex<Vec<Read>>,
        scripts: Mutex<Vec<String>>,
    }

    impl LogTransport {
        fn new(reads: Vec<Read>) -> Arc<Self> {
            Arc::new(Self {
                reads: Mutex::new(reads),
                scripts: Mutex::new(Vec::new()),
            })
        }

        fn baseline_reads(&self) -> Vec<bool> {
            self.scripts
                .lock()
                .unwrap()
                .iter()
                .map(|script| script.contains("if [ 1 -eq 1 ]"))
                .collect()
        }
    }

    impl Transport for LogTransport {
        async fn run(
            &self,
            script: &str,
            output: mpsc::UnboundedSender<String>,
        ) -> Result<i32, TransportError> {
            self.scripts.lock().unwrap().push(script.into());
            let read = {
                let mut reads = self.reads.lock().unwrap();
                if reads.len() > 1 {
                    reads.remove(0)
                } else {
                    reads[0].clone()
                }
            };
            match read {
                Read::Lost => Err(TransportError::ConnectionLost("lost".into())),
                Read::Missing => {
                    let _ = output.send("@missing".into());
                    Ok(0)
                }
                Read::File { size, start, bytes } => {
                    let engine = base64::engine::general_purpose::STANDARD;
                    let _ = output.send(format!(
                        "@file 42 {size} {start} {} {}",
                        bytes.len(),
                        u8::from(start == 0)
                    ));
                    let _ = output.send(engine.encode("storage/logs/laravel.log"));
                    let _ = output.send(engine.encode(bytes));
                    Ok(0)
                }
            }
        }

        async fn reconnect(&self) -> Result<(), TransportError> {
            Ok(())
        }
    }

    async fn observe(
        transport: &Arc<LogTransport>,
        post_window: Duration,
    ) -> (WatchResult, Vec<DeployEvent>) {
        let target = DeployTarget {
            env: "staging".into(),
            production: false,
            ssh_alias: "app".into(),
            path: "/srv/app".into(),
            branch: "main".into(),
            steps: Vec::new(),
            maintenance: false,
            watch_log: true,
            log: None,
            log_daily: false,
            smoke_url: None,
            timezone: None,
            logs: Default::default(),
        };
        let (events, mut receiver) = mpsc::unbounded_channel();
        let observer = LogObserver::start(
            transport.clone(),
            &target,
            events,
            None,
            Arc::new(AtomicBool::new(false)),
        )
        .await;
        observer.begin();
        let result = observer.finish(WatchStatus::Complete, post_window).await;
        let mut emitted = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            emitted.push(event);
        }
        (result, emitted)
    }

    fn new_error_events(events: &[DeployEvent]) -> usize {
        events
            .iter()
            .filter(|event| matches!(event, DeployEvent::NewLogError { .. }))
            .count()
    }

    fn whole(bytes: &'static [u8]) -> Read {
        Read::File {
            size: bytes.len(),
            start: 0,
            bytes,
        }
    }

    #[tokio::test]
    async fn failed_baseline_recovers_as_partial_without_reporting_old_errors() {
        let transport = LogTransport::new(vec![Read::Lost, whole(OLD_ERROR)]);
        let (result, events) = observe(&transport, Duration::ZERO).await;

        assert_eq!(transport.baseline_reads(), [true, true]);
        assert_eq!(result.status, WatchStatus::Partial);
        assert_eq!(result.baseline_signatures, 1);
        assert!(result.new_errors.is_empty());
        assert_eq!(new_error_events(&events), 0);
    }

    #[tokio::test]
    async fn file_created_after_missing_baseline_is_observed_as_new() {
        let transport = LogTransport::new(vec![Read::Missing, whole(OLD_ERROR)]);
        let (result, events) = observe(&transport, Duration::ZERO).await;

        assert_eq!(transport.baseline_reads(), [true, false]);
        assert_eq!(result.status, WatchStatus::Complete);
        assert_eq!(result.baseline_signatures, 0);
        assert_eq!(result.new_errors.len(), 1);
        assert_eq!(new_error_events(&events), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn file_created_after_failed_baseline_and_missing_read_is_new() {
        let caught_up = Read::File {
            size: NEW_ERROR.len(),
            start: NEW_ERROR.len(),
            bytes: b"",
        };
        let transport =
            LogTransport::new(vec![Read::Lost, Read::Missing, whole(NEW_ERROR), caught_up]);
        let (result, events) = observe(&transport, Duration::from_millis(100)).await;

        assert_eq!(&transport.baseline_reads()[..3], [true, true, false]);
        assert_eq!(result.status, WatchStatus::Partial);
        assert_eq!(result.new_errors.len(), 1);
        assert_eq!(result.new_errors[0].exception, "NewException");
        assert_eq!(new_error_events(&events), 1);
    }

    #[tokio::test]
    async fn watch_that_falls_behind_the_log_is_partial() {
        let behind = Read::File {
            size: 10 * NEW_ERROR.len(),
            start: 0,
            bytes: NEW_ERROR,
        };
        let transport = LogTransport::new(vec![Read::Missing, behind]);
        let (result, _) = observe(&transport, Duration::ZERO).await;

        assert_eq!(result.status, WatchStatus::Partial);
        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.contains("faster than it could be read")));
    }

    #[test]
    fn app_file_is_a_whole_path_segment_outside_vendor() {
        let file = |entry: &str| signature("ERROR", entry).2;
        assert_eq!(
            file("RuntimeException at /var/www/html/app/Http/Kernel.php:10"),
            Some("/var/www/html/app/Http/Kernel.php".into())
        );
        assert_eq!(
            file(
                "QueryException at /srv/my-app/vendor/laravel/framework/src/Connection.php:822\n\
                 #0 /srv/my-app/vendor/laravel/framework/src/Builder.php(25): run()\n\
                 #1 /srv/my-app/app/Http/Controllers/UserController.php(42): get()"
            ),
            Some("/srv/my-app/app/Http/Controllers/UserController.php".into())
        );
        assert_eq!(
            file("RuntimeException at /var/www/html/vendor/acme/pkg/app/Thing.php:5"),
            None
        );
        assert_eq!(
            signature(
                "ERROR",
                "RuntimeException at /srv/app/app/Models/User.php(7): x"
            )
            .3,
            Some("/srv/app/app/Models/User.php:7".into())
        );
    }

    #[test]
    fn history_keeps_recent_signatures_first() {
        let previous: Vec<String> = (0..MAX_HISTORY).map(|i| format!("a-{i:04}")).collect();
        let kept = recent_first(["z-recent".to_string(), "a-0003".to_string()], previous);

        assert_eq!(kept.len(), MAX_HISTORY);
        assert_eq!(&kept[..2], ["a-0003", "z-recent"]);
        assert_eq!(kept.iter().filter(|s| *s == "a-0003").count(), 1);
        assert!(kept.contains(&"a-4997".to_string()));
        assert!(!kept.contains(&"a-4999".to_string()));
    }

    #[test]
    fn curl_status_000_is_no_status() {
        assert_eq!(parse_write_out("shipslip:000 0.012"), (None, 12));
        assert_eq!(parse_write_out("shipslip:204 0.5"), (Some(204), 500));
    }

    /// Pins the deploy watch's grouping before `slip logs` starts sharing its parser.
    #[tokio::test]
    async fn watch_groups_new_errors_by_class_and_app_file() {
        const LOG: &[u8] = b"[2026-09-01 00:00:00] production.ERROR: OldException: old failure\n\
[2026-09-01 00:01:00] production.ERROR: Illuminate\\Database\\QueryException: Duplicate entry 7 at /srv/app/app/Http/Controllers/OrderController.php:42\n\
[stacktrace]\n\
#0 /srv/app/vendor/laravel/framework/src/Connection.php(776): run()\n\
#1 /srv/app/app/Http/Controllers/OrderController.php(42): store()\n\
[2026-09-01 00:02:00] production.ERROR: Illuminate\\Database\\QueryException: Duplicate entry 9 at /srv/app/app/Http/Controllers/OrderController.php:42\n\
[2026-09-01 00:03:00] production.ERROR: Illuminate\\Database\\QueryException: Deadlock found at /srv/app/app/Http/Controllers/OrderController.php:50\n\
[2026-09-01 00:04:00] production.INFO: Order shipped 12\n\
[2026-09-01 00:05:00] production.WARNING: Slow query 3000ms\n";
        let appended = Read::File {
            size: OLD_ERROR.len() + LOG.len(),
            start: OLD_ERROR.len(),
            bytes: LOG,
        };
        let transport = LogTransport::new(vec![whole(OLD_ERROR), appended]);
        let (result, _) = observe(&transport, Duration::ZERO).await;

        let file = "/srv/app/app/Http/Controllers/OrderController.php";
        let variant = |message: &str, normalized: &str, line: &str, count| ErrorVariant {
            signature: format!("ERROR|Illuminate\\Database\\QueryException|{file}|{normalized}"),
            level: "ERROR".into(),
            message: message.into(),
            display_file_line: Some(format!("{file}:{line}")),
            count,
            phase: LogPhase::During,
        };
        assert_eq!(result.status, WatchStatus::Complete);
        assert_eq!(result.baseline_signatures, 1);
        assert_eq!((result.observed_lines, result.parsed_lines), (9, 9));
        assert_eq!(
            result.new_errors,
            [ErrorGroup {
                exception: "Illuminate\\Database\\QueryException".into(),
                file: Some(file.into()),
                count: 3,
                variants: vec![
                    variant(
                        &format!("Illuminate\\Database\\QueryException: Duplicate entry 7 at {file}:42"),
                        &format!("Illuminate\\Database\\QueryException: Duplicate entry <num> at {file}:<num>"),
                        "42",
                        2,
                    ),
                    variant(
                        &format!("Illuminate\\Database\\QueryException: Deadlock found at {file}:50"),
                        &format!("Illuminate\\Database\\QueryException: Deadlock found at {file}:<num>"),
                        "50",
                        1,
                    ),
                ],
                overflow_variants: 0,
            }]
        );
    }

    /// Signatures are stored in each user's `history.json`, so their bytes are
    /// a compatibility contract: a change makes known errors look new again.
    #[test]
    fn signature_bytes_are_stable() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "ERROR",
                "Illuminate\\Database\\QueryException: SQLSTATE[23000]: Integrity constraint violation: 1062 Duplicate entry 'a@b.co' for key 'users_email_unique' (Connection: mysql, SQL: insert into `users` (`email`) values (a@b.co)) at /srv/app/vendor/laravel/framework/src/Illuminate/Database/Connection.php:822\n[stacktrace]\n#0 /srv/app/vendor/laravel/framework/src/Illuminate/Database/Connection.php(776): runQueryCallback()\n#1 /srv/app/app/Http/Controllers/UserController.php(42): store()",
                "ERROR|Illuminate\\Database\\QueryException|/srv/app/app/Http/Controllers/UserController.php|Illuminate\\Database\\QueryException: SQLSTATE[<num>]: Integrity constraint violation: <num> Duplicate entry 'a@b.co<quoted>users_email_unique' (Connection: mysql, SQL: insert into `users` (`email`) values (a@b.co)) at /srv/app/vendor/laravel/framework/src/Illuminate/Database/Connection.php:<num>",
            ),
            (
                "ERROR",
                "Order 9f1c2d3e-4b5a-6789-abcd-ef0123456789 failed after 3 retries (code 0x1F) at /srv/app/app/Jobs/ChargeOrder.php(17): handle()",
                "ERROR|Error|/srv/app/app/Jobs/ChargeOrder.php|Order <uuid> failed after <num> retries (code <num>) at /srv/app/app/Jobs/ChargeOrder.php(<num>): handle()",
            ),
            (
                "CRITICAL",
                r#"Stripe\Exception\ApiConnectionException: Could not connect to Stripe "api.stripe.com timed out" {"exception":"[object] (Stripe\\Exception\\ApiConnectionException(code: 0): x at /srv/app/app/Services/Billing.php:88)"}"#,
                r#"CRITICAL|Stripe\Exception\ApiConnectionException|/srv/app/app/Services/Billing.php|Stripe\Exception\ApiConnectionException: Could not connect to Stripe <quoted> {"exception":<quoted>}"#,
            ),
            (
                "ERROR",
                "Something went wrong for user 42",
                "ERROR|Error||Something went wrong for user <num>",
            ),
            (
                "WARNING",
                "TypeError: Argument #1 ($id) must be of type int, string given, called in /var/www/html/app/Models/User.php on line 10 at /var/www/html/app/Models/User.php:10",
                "WARNING|TypeError|/var/www/html/app/Models/User.php|TypeError: Argument #<num> ($id) must be of type int, string given, called in /var/www/html/app/Models/User.php on line <num> at /var/www/html/app/Models/User.php:<num>",
            ),
            (
                "ERROR",
                "RuntimeException: short 'id-7' value",
                "ERROR|RuntimeException||RuntimeException: short 'id-<num>' value",
            ),
            (
                "ERROR",
                "RuntimeException: long 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' value",
                "ERROR|RuntimeException||RuntimeException: long <quoted> value",
            ),
            (
                "ERROR",
                "LogicException: résumé ünïcödé 12",
                "ERROR|LogicException||LogicException: résumé ünïcödé <num>",
            ),
        ];
        for (level, entry, expected) in cases {
            assert_eq!(signature(level, entry).0, *expected, "entry: {entry}");
        }
    }

    #[test]
    fn exception_identity_comes_from_the_root_object_not_message_or_stack() {
        let cases = [
            ("Error", "Class not found"),
            ("Exception", "Something failed"),
            ("TypeError", "Invalid argument"),
            (
                "Illuminate\\Database\\QueryException",
                "SQLSTATE[HY000]: no such table",
            ),
            (
                "App\\Domain\\Failure",
                "RuntimeException mentioned in the message",
            ),
        ];
        for (class, message) in cases {
            let exception = format!("[object] ({class}(code: 0): {message} at /srv/app/app/Jobs/Export.php:16)\n[stacktrace]\n#0 /srv/app/vendor/ShareErrorsFromSession.php(48): handle()\n[previous exception] [object] (LogicException(code: 0): previous)");
            let context = serde_json::json!({"exception": exception});
            let entry = format!("RuntimeException mentioned by user {context}");
            assert_eq!(signature("ERROR", &entry).1, class, "{entry}");
            // Monolog can render JSON's escaped newlines as actual line breaks.
            let inline = entry.replace("\\n", "\n");
            assert_eq!(signature("ERROR", &inline).1, class, "{inline}");
        }
        let unicode_escape = r#"Failed {"exception":"[object] (Illuminate\u005cDatabase\u005cQueryException(code: 0): x)"}"#;
        assert_eq!(
            signature("ERROR", unicode_escape).1,
            "Illuminate\\Database\\QueryException"
        );
    }

    #[test]
    fn exception_plain_text_requires_a_complete_explicit_class() {
        for (entry, expected) in [
            ("Error: Class not found\n#0 /srv/app/vendor/ShareErrorsFromSession.php(48): handle()", "Error"),
            ("Exception(code: 0): failed", "Exception"),
            ("Illuminate\\Database\\QueryException at /srv/app/app/Jobs/Export.php:16", "Illuminate\\Database\\QueryException"),
            (r"Illuminate\\Database\\QueryException: failed", "Illuminate\\Database\\QueryException"),
            ("ShareErrorsFromSession: handle()", "Error"),
            ("QueryExceptionFactory: failed", "Error"),
            ("User mentioned QueryException in a message\n#0 /srv/app/vendor/RuntimeException.php:12", "Error"),
        ] {
            assert_eq!(signature("ERROR", entry).1, expected, "{entry}");
        }
    }

    #[test]
    fn exception_watch_preserves_baseline_when_middleware_changes() {
        let header = "[2026-10-02 12:00:00] production.ERROR: Class not found {\"exception\":\"[object] (Error(code: 0): Class not found at /srv/app/app/Jobs/Export.php:16)\n[stacktrace]\n";
        let baseline = format!("{header}#0 /srv/app/vendor/Handler.php(48): handle()\n\"}}\n");
        let current =
            format!("{header}#0 /srv/app/vendor/ShareErrorsFromSession.php(48): handle()\n\"}}\n");
        let (events, mut receiver) = mpsc::unbounded_channel();
        let mut state = LogState::new(Vec::new());
        state.bytes(baseline.as_bytes(), LogPhase::During, true, &events);
        state.flush(true, &events);
        state.bytes(current.as_bytes(), LogPhase::After, false, &events);
        state.flush(false, &events);
        assert!(state.result.new_errors.is_empty());
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn exception_watch_groups_escaped_and_plain_classes_together() {
        let exception = "[object] (Illuminate\\Database\\QueryException(code: 0): Missing table at /srv/app/app/Jobs/Export.php:16)";
        let context = serde_json::json!({"exception": exception});
        let entries = format!("[2026-10-02 12:00:00] production.ERROR: Missing table {context}\n[2026-10-02 12:00:01] production.ERROR: Illuminate\\Database\\QueryException: Missing table at /srv/app/app/Jobs/Export.php:16\n");
        let (events, _receiver) = mpsc::unbounded_channel();
        let mut state = LogState::new(Vec::new());
        state.bytes(entries.as_bytes(), LogPhase::After, false, &events);
        state.flush(false, &events);
        assert_eq!(state.result.new_errors.len(), 1);
        assert_eq!(
            state.result.new_errors[0].exception,
            "Illuminate\\Database\\QueryException"
        );
        assert_eq!(state.result.new_errors[0].count, 2);
    }
}
