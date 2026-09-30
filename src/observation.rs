//! Bounded Laravel log observation and a local HTTP smoke check.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
const ENTRY_BYTES: usize = 256 * 1024;
const VIEW_BYTES: usize = 10 * 1024 * 1024;
const MAX_SIGNATURES: usize = 10_000;
const MAX_GROUPS: usize = 500;
const MAX_VARIANTS: usize = 50;
const POLL_EVERY: Duration = Duration::from_millis(500);
const MAX_HISTORY_BYTES: u64 = 16 * 1024 * 1024;
static HISTORY_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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
            Some(Err(error)) => (HashSet::new(), Some(error)),
            None => (HashSet::new(), None),
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
                state.flush(LogPhase::During, true, &events);
                state.partial_line.clear();
                state.cursor = Cursor {
                    path,
                    inode,
                    offset: size,
                };
            }
            Fetch::Missing => {}
            Fetch::Unavailable(reason) => {
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
        let default = if target.log_daily {
            "storage/logs/laravel"
        } else {
            "storage/logs/laravel.log"
        };
        Self {
            app_path: target.path.clone(),
            path: target.log.as_deref().unwrap_or(default).into(),
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
    baseline: HashSet<String>,
    seen: HashSet<String>,
    group_index: HashMap<(String, Option<String>), usize>,
    overflow_group_keys: HashSet<(String, Option<String>)>,
    view: VecDeque<String>,
    view_bytes: usize,
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
    started: Instant,
}

impl LogState {
    fn warn(&mut self, warning: impl Into<String>) {
        let warning = warning.into();
        if self.result.warnings.len() < 20 && !self.result.warnings.contains(&warning) {
            self.result.warnings.push(warning);
        }
    }

    fn new(history: HashSet<String>) -> Self {
        let mut result = WatchResult::not_run();
        result.history_signatures = history.len();
        Self {
            result,
            known: history,
            baseline: HashSet::new(),
            seen: HashSet::new(),
            group_index: HashMap::new(),
            overflow_group_keys: HashSet::new(),
            view: VecDeque::new(),
            view_bytes: 0,
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
            self.view_bytes += line.len();
            self.view.push_back(line.clone());
            while self.view_bytes > VIEW_BYTES {
                if let Some(old) = self.view.pop_front() {
                    self.view_bytes -= old.len();
                    self.result.dropped_view_lines += 1;
                }
            }
        }
        if let Some(header) = parse_header(&line) {
            self.flush(phase, baseline, events);
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

    fn flush(
        &mut self,
        _phase: LogPhase,
        baseline: bool,
        events: &mpsc::UnboundedSender<DeployEvent>,
    ) {
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
    ) -> (WatchResult, HashSet<String>) {
        if requested == WatchStatus::NotRun {
            return (WatchResult::not_run(), HashSet::new());
        }
        self.flush(LogPhase::After, false, events);
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
        let mut signatures = self.known;
        signatures.extend(self.seen);
        (self.result, signatures)
    }
}

#[derive(Clone)]
struct Header {
    level: String,
    message: String,
}

fn parse_header(line: &str) -> Option<Header> {
    if !line.starts_with('[') {
        return None;
    }
    let rest = line.split_once("] ")?.1;
    let (level_part, message) = rest.split_once(": ")?;
    let level = level_part.rsplit('.').next()?.to_ascii_uppercase();
    if !matches!(
        level.as_str(),
        "DEBUG" | "INFO" | "NOTICE" | "WARNING" | "ERROR" | "CRITICAL" | "ALERT" | "EMERGENCY"
    ) {
        return None;
    }
    Some(Header {
        level,
        message: message.into(),
    })
}

fn patterns() -> &'static (Regex, Regex, Regex, Regex, Regex, Regex) {
    static RE: OnceLock<(Regex, Regex, Regex, Regex, Regex, Regex)> = OnceLock::new();
    RE.get_or_init(|| {
        (
            Regex::new(r"(?i)([A-Za-z_\\][A-Za-z0-9_\\]*(?:Exception|Error))").unwrap(),
            Regex::new(r"([A-Za-z0-9_./-]*app/[A-Za-z0-9_./-]+\.php):(\d+)").unwrap(),
            Regex::new(r"[0-9a-fA-F]{8}-[0-9a-fA-F-]{27,}").unwrap(),
            Regex::new(r"0x[0-9a-fA-F]+|\b\d+\b").unwrap(),
            Regex::new(r"'[^']{65,}'|'[^']*\s[^']*'").unwrap(),
            Regex::new(r#""[^"]{65,}"|"[^"]*\s[^"]*""#).unwrap(),
        )
    })
}

fn signature(level: &str, entry: &str) -> (String, String, Option<String>, Option<String>) {
    let (class_re, frame_re, uuid_re, number_re, single_quote_re, double_quote_re) = patterns();
    let class = class_re
        .find(entry)
        .map(|m| clip_utf8(m.as_str(), 256))
        .unwrap_or_else(|| "Error".into());
    let frame = frame_re.captures(entry);
    let file = frame.as_ref().map(|m| clip_utf8(&m[1], 512));
    let display = frame
        .as_ref()
        .map(|m| format!("{}:{}", clip_utf8(&m[1], 512), &m[2]));
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

fn clip_utf8(value: &str, limit: usize) -> String {
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
        if !catch_up {
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
        match tokio::time::timeout(
            Duration::from_secs(10),
            fetch(&*transport, &spec, &state.cursor, false),
        )
        .await
        .unwrap_or_else(|_| Fetch::Unavailable("log poll timed out".into()))
        {
            Fetch::File {
                path,
                inode,
                size: _,
                start,
                bytes,
                rotated,
            } => {
                if rotated && state.saw_file {
                    state.partial = true;
                    state.warn("Log rotated or truncated during watch");
                    state.partial_line.clear();
                    state.flush(phase, false, &events);
                }
                state.saw_file = true;
                state.unavailable = false;
                state.result.log_path = Some(path.clone());
                state.cursor = Cursor {
                    path,
                    inode,
                    offset: start + bytes.len() as u64,
                };
                state.bytes(&bytes, phase, false, &events);
                if bytes.is_empty()
                    && state.current_entry.is_some()
                    && state.last_entry_update.elapsed() >= Duration::from_secs(1)
                {
                    state.flush(phase, false, &events);
                }
            }
            Fetch::Missing => {}
            Fetch::Unavailable(reason) => {
                state.partial = true;
                state.unavailable = true;
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

fn read_history(path: &Path) -> Result<HashSet<String>, std::io::Error> {
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
            Ok(values.into_iter().take(5_000).collect())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(HashSet::new()),
        Err(error) => Err(error),
    }
}

fn write_history(path: &Path, signatures: &HashSet<String>) -> Result<(), std::io::Error> {
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
    let mut values: Vec<_> = signatures.iter().collect();
    values.sort();
    values.truncate(5_000);
    let bytes = serde_json::to_vec(&values)?;
    let temp = path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        HISTORY_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    if let Err(error) = std::io::Write::write_all(&mut file, &bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| std::fs::rename(&temp, path))
    {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

pub async fn smoke_check(url: Option<&str>) -> SmokeResult {
    let Some(url) = url else {
        return SmokeResult::NotConfigured;
    };
    let command = tokio::process::Command::new("curl")
        .args([
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
            let text = String::from_utf8_lossy(&output.stdout);
            let parts: Vec<_> = text
                .trim()
                .strip_prefix("shipslip:")
                .unwrap_or("")
                .split_whitespace()
                .collect();
            let status = parts.first().and_then(|code| code.parse::<u16>().ok());
            let latency_ms = parts
                .get(1)
                .and_then(|seconds| seconds.parse::<f64>().ok())
                .map(|seconds| (seconds * 1000.0) as u64)
                .unwrap_or(0);
            if output.status.success() && status.is_some_and(|code| (200..300).contains(&code)) {
                SmokeResult::Passed {
                    status: status.unwrap(),
                    latency_ms,
                }
            } else {
                let reason = if output.status.success() {
                    format!("HTTP {}", status.unwrap_or(0))
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
