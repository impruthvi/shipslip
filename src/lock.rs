//! The deploy lock: `<git dir>/shipslip.lock`, one per app on the server.
//!
//! A run owns the lock while `<lock>/run_id` holds its run id. Every write
//! checks that first, and release, heartbeat and break never act on a lock
//! that another run has taken over.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::runner::run_collect;
use crate::script::shell_quote;
use crate::transport::{Transport, TransportError};

/// A lock whose heartbeat is this old may be broken.
pub const STALE_AFTER: Duration = Duration::from_secs(120);

pub(crate) const HEARTBEAT_EVERY: Duration = Duration::from_secs(30);

/// Who holds a lock, as written by the holder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockOwner {
    pub run_id: String,
    pub user: String,
    pub machine: String,
    pub pid: u32,
    /// Unix seconds, holder's clock.
    pub started_at: u64,
    pub target_sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockInfo {
    /// `None` if the holder's details could not be read.
    pub owner: Option<LockOwner>,
    /// Seconds since the last heartbeat, by the server's clock.
    pub age_secs: u64,
}

impl LockInfo {
    pub fn is_stale(&self) -> bool {
        self.age_secs >= STALE_AFTER.as_secs()
    }
}

impl fmt::Display for LockInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.owner {
            Some(o) => write!(
                f,
                "held by {}@{} deploying {}, last heartbeat {}s ago",
                o.user, o.machine, o.target_sha, self.age_secs
            ),
            None => write!(f, "held, last heartbeat {}s ago", self.age_secs),
        }
    }
}

impl LockOwner {
    pub(crate) fn current(run_id: &str, target_sha: &str) -> Self {
        let machine = std::process::Command::new("hostname")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into());
        Self {
            run_id: run_id.into(),
            user: std::env::var("USER").unwrap_or_else(|_| "unknown".into()),
            machine,
            pid: std::process::id(),
            started_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default(),
            target_sha: target_sha.into(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Acquire {
    Acquired,
    Held(LockInfo),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Break {
    Broken,
    NotHeld,
    Live(u64),
}

/// Runs `body` in the app's git dir context with `$l` set to the lock path
/// and `$r` to this run's id.
fn script(path: &str, run_id: &str, body: &str) -> String {
    format!(
        "cd {} || exit $?\n\
         l=\"$(git rev-parse --absolute-git-dir)/shipslip.lock\" || exit $?\n\
         r={}\n\
         owned() {{ [ \"$(cat \"$1/run_id\" 2>/dev/null)\" = \"$r\" ]; }}\n\
         {body}",
        shell_quote(path),
        shell_quote(run_id),
    )
}

pub(crate) fn acquire_script(path: &str, owner: &LockOwner) -> String {
    let json = serde_json::to_string(owner).expect("owner serializes");
    script(
        path,
        &owner.run_id,
        &format!(
            r#"if mkdir "$l" 2>/dev/null; then
  date +%s > "$l/heartbeat"
  printf '%s' {owner} > "$l/owner.json"
  printf '%s\n' "$r" > "$l/run_id"
  echo acquired
else
  echo "held $(( $(date +%s) - $(cat "$l/heartbeat" 2>/dev/null || echo 0) )) $(cat "$l/owner.json" 2>/dev/null)"
fi
"#,
            owner = shell_quote(&json),
        ),
    )
}

pub(crate) fn check_script(path: &str, run_id: &str) -> String {
    script(
        path,
        run_id,
        "if owned \"$l\"; then echo owned; else echo lost; fi\n",
    )
}

// `cd` first: if the lock is renamed (broken) meanwhile, the write lands in
// the renamed dir, never in a successor's lock.
pub(crate) fn heartbeat_script(path: &str, run_id: &str) -> String {
    script(
        path,
        run_id,
        r#"if cd "$l" 2>/dev/null && owned . && date +%s > heartbeat.tmp && mv heartbeat.tmp heartbeat && owned "$l"; then
  echo owned
else
  echo lost
fi
"#,
    )
}

pub(crate) fn release_script(path: &str, run_id: &str) -> String {
    script(
        path,
        run_id,
        r#"owned "$l" && mv -T "$l" "$l.released-$r" 2>/dev/null || { echo not-owner; exit 0; }
if owned "$l.released-$r"; then
  rm -rf "$l.released-$r"
  echo released
else
  mv -T "$l.released-$r" "$l" 2>/dev/null
  echo not-owner
fi
"#,
    )
}

pub(crate) fn break_script(path: &str) -> String {
    script(
        path,
        "",
        &format!(
            r#"[ -d "$l" ] || {{ echo not-held; exit 0; }}
age=$(( $(date +%s) - $(cat "$l/heartbeat" 2>/dev/null || echo 0) ))
if [ "$age" -lt {stale} ]; then echo "live $age"; exit 0; fi
mv -T "$l" "$l.broken-$(date +%s)-$$" && echo broken
"#,
            stale = STALE_AFTER.as_secs(),
        ),
    )
}

fn last_line(lines: &[String]) -> Option<&str> {
    lines.iter().rev().map(|l| l.trim()).find(|l| !l.is_empty())
}

fn parse_acquire(lines: &[String]) -> Option<Acquire> {
    let last = last_line(lines)?;
    if last == "acquired" {
        return Some(Acquire::Acquired);
    }
    let rest = last.strip_prefix("held ")?;
    let (age, owner) = rest.split_once(' ').unwrap_or((rest, ""));
    Some(Acquire::Held(LockInfo {
        owner: serde_json::from_str(owner).ok(),
        age_secs: age.parse().ok()?,
    }))
}

fn parse_break(lines: &[String]) -> Option<Break> {
    match last_line(lines)? {
        "broken" => Some(Break::Broken),
        "not-held" => Some(Break::NotHeld),
        other => other.strip_prefix("live ")?.parse().ok().map(Break::Live),
    }
}

/// Runs a lock script; `Err` carries a description of what went wrong.
async fn run<T: Transport>(transport: &T, script: &str) -> Result<Vec<String>, String> {
    match run_collect(transport, script).await {
        (Ok(0), lines) => Ok(lines),
        (Ok(code), lines) => Err(format!("exited with {code}: {}", lines.join("\n"))),
        (Err(e), _) => Err(e.to_string()),
    }
}

pub(crate) async fn acquire<T: Transport>(
    transport: &T,
    path: &str,
    owner: &LockOwner,
) -> Result<Acquire, String> {
    let lines = run(transport, &acquire_script(path, owner)).await?;
    parse_acquire(&lines).ok_or_else(|| format!("unexpected output: {}", lines.join("\n")))
}

pub(crate) async fn is_owned<T: Transport>(
    transport: &T,
    path: &str,
    run_id: &str,
) -> Result<bool, String> {
    let lines = run(transport, &check_script(path, run_id)).await?;
    match last_line(&lines) {
        Some("owned") => Ok(true),
        Some("lost") => Ok(false),
        _ => Err(format!("unexpected output: {}", lines.join("\n"))),
    }
}

pub(crate) async fn heartbeat<T: Transport>(transport: &T, path: &str, run_id: &str) {
    let _ = run(transport, &heartbeat_script(path, run_id)).await;
}

/// Releases the lock if this run still owns it. Leaves it alone otherwise.
pub(crate) async fn release<T: Transport>(
    transport: &T,
    path: &str,
    run_id: &str,
) -> Result<(), TransportError> {
    match run_collect(transport, &release_script(path, run_id)).await {
        (Err(e), _) => Err(e),
        _ => Ok(()),
    }
}

pub(crate) async fn break_stale<T: Transport>(transport: &T, path: &str) -> Result<Break, String> {
    let lines = run(transport, &break_script(path)).await?;
    parse_break(&lines).ok_or_else(|| format!("unexpected output: {}", lines.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn owner() -> LockOwner {
        LockOwner {
            run_id: "run-1".into(),
            user: "ana".into(),
            machine: "mac".into(),
            pid: 7,
            started_at: 1,
            target_sha: "abc".into(),
        }
    }

    #[test]
    fn acquire_output_is_parsed() {
        assert_eq!(
            parse_acquire(&lines(&["motd", "acquired"])),
            Some(Acquire::Acquired)
        );
        let json = serde_json::to_string(&owner()).unwrap();
        assert_eq!(
            parse_acquire(&lines(&[&format!("held 42 {json}")])),
            Some(Acquire::Held(LockInfo {
                owner: Some(owner()),
                age_secs: 42
            }))
        );
        assert_eq!(
            parse_acquire(&lines(&["held 300 "])),
            Some(Acquire::Held(LockInfo {
                owner: None,
                age_secs: 300
            }))
        );
        assert_eq!(parse_acquire(&lines(&["held x"])), None);
    }

    #[test]
    fn stale_after_two_minutes() {
        let info = |age_secs| LockInfo {
            owner: None,
            age_secs,
        };
        assert!(!info(119).is_stale());
        assert!(info(120).is_stale());
    }

    #[test]
    fn owner_json_is_quoted_into_the_script() {
        let mut o = owner();
        o.user = "it's".into();
        let s = acquire_script("/app", &o);
        assert!(s.contains("r='run-1'\n"));
        assert!(s.contains(r#""user":"it'\''s""#));
    }

    #[test]
    fn release_checks_ownership_before_and_after_moving() {
        let s = release_script("/app", "run-1");
        let first = s.find("owned \"$l\" && mv").unwrap();
        let second = s.find("if owned \"$l.released-$r\"").unwrap();
        assert!(first < second && second < s.find("rm -rf").unwrap());
    }

    #[test]
    fn break_output_is_parsed() {
        assert_eq!(parse_break(&lines(&["broken"])), Some(Break::Broken));
        assert_eq!(parse_break(&lines(&["not-held"])), Some(Break::NotHeld));
        assert_eq!(parse_break(&lines(&["live 12"])), Some(Break::Live(12)));
    }
}
