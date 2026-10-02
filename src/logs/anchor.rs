//! Anchor selection: an active lock wins, then a verified run, then checkout
//! history, then 24h. Missing and failed reads remain distinct in the headline.

use super::WindowTime;
use crate::marker::{decode, Marker};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Probe {
    pub marker: Option<Result<Marker, String>>,
    pub head: Option<Result<String, String>>,
    pub reflog: Option<Result<String, String>>,
    pub active: Option<(String, Option<usize>)>,
    pub times: HashMap<String, WindowTime>,
    pub splits: HashMap<String, (u64, String)>,
}

pub(super) struct Choice {
    pub label: String,
    pub time: WindowTime,
    pub marker: Option<Marker>,
    pub warnings: Vec<String>,
}

pub(super) fn reflog_fields(tip: &str) -> Option<(&str, u64)> {
    let mut fields = tip.split_whitespace();
    let sha = fields.next()?;
    if !crate::script::is_full_sha(sha) {
        return None;
    }
    let time = fields
        .next()?
        .strip_prefix("HEAD@{")?
        .strip_suffix('}')?
        .parse()
        .ok()?;
    Some((sha, time))
}

impl Probe {
    pub fn line(&mut self, line: &str, fallback: &WindowTime) -> bool {
        let fields: Vec<_> = line.split(' ').collect();
        match fields.as_slice() {
            ["@marker", "missing"] => {}
            ["@marker", "ok", text] => {
                self.marker = Some(
                    decode(text)
                        .and_then(|text| Marker::parse(&text))
                        .ok_or_else(|| "last-run marker is corrupt".into()),
                )
            }
            ["@marker", "error", error] => {
                self.marker = Some(Err(
                    decode(error).unwrap_or_else(|| "could not read last-run marker".into())
                ))
            }
            ["@git-head", "ok", sha] => {
                self.head = Some(if crate::script::is_full_sha(sha) {
                    Ok(sha.to_string())
                } else {
                    Err("invalid HEAD".into())
                })
            }
            ["@git-head", "error", error] => {
                self.head = Some(Err(decode(error).unwrap_or_else(|| "git failed".into())))
            }
            ["@reflog", "missing"] => {}
            ["@reflog", "ok", tip] => {
                self.reflog = Some(
                    decode(tip)
                        .filter(|tip| reflog_fields(tip).is_some())
                        .ok_or_else(|| "invalid reflog output".into()),
                )
            }
            ["@reflog", "error", error] => {
                self.reflog = Some(Err(decode(error).unwrap_or_else(|| "reflog failed".into())))
            }
            ["@active", run, step] => self.active = decode(run).map(|run| (run, step.parse().ok())),
            ["@anchor-time", key, epoch, date, clock, offset] => {
                if let Ok(start) = epoch.parse() {
                    self.times.insert(
                        key.to_string(),
                        WindowTime {
                            now: fallback.now,
                            start,
                            start_local: format!("{date} {clock}"),
                            start_date: date.to_string(),
                            offset_start: offset.to_string(),
                            offset_now: fallback.offset_now.clone(),
                        },
                    );
                }
            }
            ["@split", path, size, sum] => {
                if let (Some(path), Ok(size)) = (decode(path), size.parse()) {
                    self.splits.insert(path, (size, sum.to_string()));
                }
            }
            _ => return false,
        }
        true
    }

    pub fn choose(&self, fallback: WindowTime) -> Choice {
        let mut warnings = Vec::new();
        if let Some(Ok(marker)) = &self.marker {
            if let Some(time) = self
                .times
                .get("marker")
                .filter(|time| time.start <= fallback.now && time.start == marker.started_at)
            {
                let short = &marker.target_sha[..7];
                if let Some((_, step)) = self
                    .active
                    .as_ref()
                    .filter(|(run, _)| run == &marker.run_id)
                {
                    return Choice {
                        label: format!(
                            "deploy in progress (run {}{}); since run start",
                            marker.run_id,
                            step.map(|step| format!(", step {step}"))
                                .unwrap_or_default()
                        ),
                        time: time.clone(),
                        marker: Some(marker.clone()),
                        warnings,
                    };
                }
                let git_error = self
                    .head
                    .as_ref()
                    .and_then(|head| head.as_ref().err())
                    .or_else(|| self.reflog.as_ref().and_then(|tip| tip.as_ref().err()));
                if let Some(error) = git_error {
                    return Choice {
                        label: format!(
                            "since slip {} {} ({short}, unverified: {error})",
                            marker.plan, marker.run_id
                        ),
                        time: time.clone(),
                        marker: Some(marker.clone()),
                        warnings,
                    };
                }
                let head = self.head.as_ref().and_then(|head| head.as_ref().ok());
                let tip = self.reflog.as_ref().and_then(|tip| tip.as_ref().ok());
                let valid = head == Some(&marker.target_sha)
                    && match &marker.reflog_tip {
                        Some(stored) => tip == Some(stored),
                        None => tip
                            .and_then(|tip| reflog_fields(tip))
                            .is_some_and(|(sha, _)| sha == marker.target_sha),
                    };
                if valid {
                    let outcome = if marker.reflog_tip.is_some() {
                        marker.outcome.as_deref().unwrap_or("outcome unknown")
                    } else {
                        "outcome unknown"
                    };
                    return Choice {
                        label: format!(
                            "since slip {} {} ({short}, {outcome})",
                            marker.plan, marker.run_id
                        ),
                        time: time.clone(),
                        marker: Some(marker.clone()),
                        warnings,
                    };
                }
                warnings.push(if tip.is_none() {
                    format!(
                        "run {} could not be verified: reflog is missing",
                        marker.run_id
                    )
                } else if head != Some(&marker.target_sha)
                    && marker
                        .reflog_tip
                        .as_deref()
                        .and_then(reflog_fields)
                        .is_none_or(|(sha, _)| sha != marker.target_sha)
                {
                    format!("run {} did not reach its target {short}", marker.run_id)
                } else {
                    format!("checkout changed after run {}", marker.run_id)
                });
            } else {
                warnings.push("last-run marker has an invalid start time".into());
            }
        } else if let Some(Err(error)) = &self.marker {
            warnings.push(format!("anchor: {error}"));
        }
        if let Some(Ok(tip)) = &self.reflog {
            if let (Some((sha, epoch)), Some(time)) = (reflog_fields(tip), self.times.get("reflog"))
            {
                if epoch == time.start && epoch <= fallback.now {
                    return Choice {
                        label: format!("since checkout change {}", &sha[..7]),
                        time: time.clone(),
                        marker: None,
                        warnings,
                    };
                }
            }
        }
        if let Some(error) = self
            .reflog
            .as_ref()
            .and_then(|tip| tip.as_ref().err())
            .or_else(|| self.head.as_ref().and_then(|head| head.as_ref().err()))
        {
            warnings.push(format!("anchor: reflog failed ({error}) — using 24h"));
        }
        Choice {
            label: "since 24h (default)".into(),
            time: fallback,
            marker: None,
            warnings,
        }
    }
}

/// Does not write even temporary files. Marker reads and Git diagnostics are
/// bounded and base64-framed. Cursor checksums are computed only inside emit's
/// already-approved file descriptor in the surrounding probe.
pub(super) const SCRIPT: &str = r#"
gitdir=$(git rev-parse --absolute-git-dir 2>/dev/null) || gitdir=''
if [ -z "$gitdir" ]; then
  if [ -d .git ]; then gitdir=.git; else gitdir=$(git rev-parse --resolve-git-dir .git 2>/dev/null) || gitdir=''; fi
fi
anchor_time() {
  [[ "$2" =~ ^[0-9]{1,12}$ ]] || return 0
  local converted
  converted=$(TZ="$tz" date -d "@$2" '+%F %T %z' 2>/dev/null) || return 0
  echo "@anchor-time $1 $2 $converted"
}
declare -A marker_offsets marker_inodes marker_sums
if [ -n "$gitdir" ] && [ -e "$gitdir/shipslip.last-run" ]; then
  if marker_text=$(head -c 262145 -- "$gitdir/shipslip.last-run" 2>/dev/null); then
    echo "@marker ok $(printf '%s' "$marker_text" | base64 -w0)"
    while IFS=' ' read -r kind a b c d; do
      case "$kind" in
        @started) anchor_time marker "$a" ;;
        @file)
          if [[ "$a" =~ ^[0-9]+$ && "$b" =~ ^[0-9]+$ && "$c" =~ ^[0-9a-f]{64}$ ]]; then
            [[ "$d" =~ ^[A-Za-z0-9+/=]+$ ]] || continue
            marker_offsets["$d"]=$b; marker_inodes["$d"]=$a; marker_sums["$d"]=$c
          fi ;;
      esac
    done <<< "$marker_text"
  else echo "@marker error $(printf '%s' 'could not read last-run marker' | base64 -w0)"; fi
else echo '@marker missing'; fi
if head=$(git rev-parse HEAD 2>&1); then echo "@git-head ok $head"; else echo "@git-head error $(printf '%s' "${head%%$'\n'*}" | base64 -w0)"; fi
if tip=$(git reflog -1 --date=unix --format='%H %gD %gs' 2>&1); then
  if [ -n "$tip" ]; then
    echo "@reflog ok $(printf '%s' "$tip" | base64 -w0)"
    at=${tip#*HEAD@\{}; at=${at%%\}*}; anchor_time reflog "$at"
  else echo '@reflog missing'; fi
else echo "@reflog error $(printf '%s' "${tip%%$'\n'*}" | base64 -w0)"; fi
if [ -n "$gitdir" ] && [ -d "$gitdir/shipslip.lock" ]; then
  if active=$(cat "$gitdir/shipslip.lock/run_id" 2>/dev/null); then
    step='-'
    if [[ "$active" =~ ^[a-zA-Z0-9_-]+$ ]]; then
      if [ -d "$HOME/.shipslip/runs/$active/maintenance-down" ] && [ ! -e "$HOME/.shipslip/runs/$active/maintenance-down/exit" ]; then step=0; fi
      for d in "$HOME"/.shipslip/runs/"$active"/step-*; do
        if [ -d "$d" ] && [ ! -e "$d/exit" ]; then step=${d##*step-}; fi
      done
    fi
    echo "@active $(printf '%s' "$active" | base64 -w0) $step"
  fi
fi
"#;

#[cfg(test)]
mod tests {
    use super::*;
    fn time() -> WindowTime {
        WindowTime {
            now: 1000,
            start: 900,
            start_local: "2026-10-02 00:00:00".into(),
            start_date: "2026-10-02".into(),
            offset_start: "+0000".into(),
            offset_now: "+0000".into(),
        }
    }
    fn probe() -> Probe {
        let sha = "a".repeat(40);
        let tip = format!("{sha} HEAD@{{900}} merge: fast-forward");
        Probe {
            marker: Some(Ok(Marker {
                run_id: "r-one".into(),
                plan: "deploy".into(),
                target_sha: sha.clone(),
                started_at: 900,
                finished_at: Some(950),
                outcome: Some("succeeded".into()),
                reflog_tip: Some(tip.clone()),
                files: Vec::new(),
            })),
            head: Some(Ok(sha)),
            reflog: Some(Ok(tip)),
            times: HashMap::from([("marker".into(), time()), ("reflog".into(), time())]),
            ..Probe::default()
        }
    }
    #[test]
    fn active_lock_wins_even_before_the_target_commit_is_checked_out() {
        let mut probe = probe();
        probe.head = Some(Ok("b".repeat(40)));
        probe.active = Some(("r-one".into(), Some(2)));
        let choice = probe.choose(time());
        assert!(choice
            .label
            .contains("deploy in progress (run r-one, step 2)"));
        assert!(choice.marker.is_some());
    }
    #[test]
    fn verifies_tip_and_labels_git_errors_and_missing_tips() {
        let mut probe = probe();
        assert!(probe.choose(time()).label.contains("succeeded"));
        probe.reflog = Some(Ok(format!(
            "{} HEAD@{{900}} checkout: moved back",
            "a".repeat(40)
        )));
        let choice = probe.choose(time());
        assert!(choice.label.starts_with("since checkout change"));
        assert!(choice.warnings[0].contains("checkout changed after run"));
        probe.head = Some(Err("dubious ownership".into()));
        assert!(probe
            .choose(time())
            .label
            .contains("unverified: dubious ownership"));
        probe.head = Some(Ok("a".repeat(40)));
        probe.marker.as_mut().unwrap().as_mut().unwrap().reflog_tip = None;
        assert!(probe.choose(time()).label.contains("outcome unknown"));
    }
    #[test]
    fn corrupt_markers_and_failed_reflogs_are_not_silently_missing() {
        let mut probe = probe();
        probe.marker = Some(Err("corrupt".into()));
        assert!(probe.choose(time()).warnings[0].contains("corrupt"));
        probe.reflog = Some(Err("git refused".into()));
        let choice = probe.choose(time());
        assert_eq!(choice.label, "since 24h (default)");
        assert!(choice
            .warnings
            .iter()
            .any(|warning| warning.contains("reflog failed (git refused)")));
    }
}
