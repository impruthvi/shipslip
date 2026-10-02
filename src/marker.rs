//! Last-run anchors. A versioned line format avoids needing PHP or jq remotely.
//! Paths and Git output are base64; all publication is atomic and owner-guarded.

use crate::runner::run_collect;
use crate::script::shell_quote;
use crate::transport::Transport;
use crate::{DeployTarget, RunPlan};
use base64::Engine as _;

const MAX_MARKER_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct File {
    pub path: String,
    pub inode: u64,
    pub size: u64,
    pub checksum: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Marker {
    pub run_id: String,
    pub plan: String,
    pub target_sha: String,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub outcome: Option<String>,
    pub reflog_tip: Option<String>,
    pub files: Vec<File>,
}

pub(crate) fn decode(value: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value)
        .ok()?;
    String::from_utf8(bytes).ok()
}

pub(crate) fn encode(value: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(value)
}

impl Marker {
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() > MAX_MARKER_BYTES {
            return None;
        }
        let mut lines = text.lines();
        if lines.next()? != "@marker 1" {
            return None;
        }
        let run_id = decode(lines.next()?.strip_prefix("@run ")?)?;
        // The run id also locates runner files; keep it a single safe component.
        if run_id.is_empty()
            || !run_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return None;
        }
        let plan = lines.next()?.strip_prefix("@plan ")?.to_string();
        if plan != "deploy"
            && plan != "rerun"
            && !plan
                .strip_prefix("from-step ")
                .is_some_and(|n| n.parse::<usize>().is_ok_and(|n| n > 0))
        {
            return None;
        }
        let target_sha = lines.next()?.strip_prefix("@target ")?.to_string();
        if !crate::script::is_full_sha(&target_sha) {
            return None;
        }
        let started_at = lines.next()?.strip_prefix("@started ")?.parse().ok()?;
        let mut marker = Self {
            run_id,
            plan,
            target_sha,
            started_at,
            finished_at: None,
            outcome: None,
            reflog_tip: None,
            files: Vec::new(),
        };
        let mut ended = false;
        for line in lines {
            let fields: Vec<_> = line.split(' ').collect();
            match fields.as_slice() {
                ["@file", inode, size, sum, path] if !ended && marker.files.len() < 50 => {
                    if sum.len() != 64 || !sum.bytes().all(|b| b.is_ascii_hexdigit()) {
                        return None;
                    }
                    let path = decode(path)?;
                    if path.is_empty()
                        || path.contains('\0')
                        || marker.files.iter().any(|f| f.path == path)
                    {
                        return None;
                    }
                    marker.files.push(File {
                        path,
                        inode: inode.parse().ok()?,
                        size: size.parse().ok()?,
                        checksum: sum.to_string(),
                    });
                }
                ["@end"] if !ended => ended = true,
                ["@finished", at, outcome] if ended && marker.finished_at.is_none() => {
                    let at = at.parse().ok()?;
                    if at < started_at || !matches!(*outcome, "succeeded" | "failed" | "cancelled")
                    {
                        return None;
                    }
                    marker.finished_at = Some(at);
                    marker.outcome = Some(outcome.to_string());
                }
                ["@reflog", tip] if ended && marker.reflog_tip.is_none() => {
                    marker.reflog_tip = Some(decode(tip)?)
                }
                _ => return None,
            }
        }
        ended.then_some(marker)
    }
}

pub(crate) fn start_script(
    target: &DeployTarget,
    run_id: &str,
    plan: RunPlan,
    sha: &str,
) -> String {
    let plan = match plan {
        RunPlan::Deploy => "deploy".into(),
        RunPlan::Rerun => "rerun".into(),
        RunPlan::FromStep(n) => format!("from-step {n}"),
    };
    let (files, overrides) = crate::logs::marker_candidates(target);
    let files = files.replace("|| exit 1", "|| return 1");
    let body = format!(
        r#"# @marker-start
owned "$l" || exit 0
m="${{l%/*}}/shipslip.last-run"
tmp="$l/marker-start-$$"
write_marker() {{
  umask 077
  printf '@marker 1\n@run %s\n@plan %s\n@target %s\n@started %s\n' {run} {plan} {sha} "$(date +%s)" > "$tmp" || return 1
  seen=' '; count_files=0
  emit() {{
    key=${{1##*/}}; key=${{key%.log}}
    if [[ "$key" =~ ^(.+)-[0-9]{{4}}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])$ ]]; then key=${{BASH_REMATCH[1]}}; fi
    key=${{2:-$key}}
    case "$key" in
{overrides}      *) ;; esac
    [ "$count_files" -lt 50 ] || return 0
    [ -f "$1" ] || return 0
    exec 3< "$1" 2>/dev/null || return 1
    fd="/proc/$$/fd/3"
    rp=$(realpath -e -- "$fd") || return 1
    encoded=$(printf '%s' "$rp" | base64 -w0)
    case "$seen" in *" $encoded "*) exec 3<&-; return 0 ;; esac
    seen="$seen$encoded "
    meta=$(stat -Lc '%i %s' -- "$fd") || return 1
    path=$(printf '%s' "$1" | base64 -w0)
    set -- $meta
    skip=$(( $2 > 256 ? $2 - 256 : 0 ))
    sum=$(dd if="$fd" iflag=skip_bytes,count_bytes skip="$skip" count="$(( $2 - skip ))" status=none | sha256sum) || return 1
    printf '@file %s %s %s %s\n' "$1" "$2" "${{sum%% *}}" "$path" >> "$tmp" || return 1
    count_files=$((count_files + 1))
    exec 3<&-
  }}
  {files}
  printf '@end\n' >> "$tmp" || return 1
  owned "$l" || return 0
  mv -T -- "$tmp" "$m" || return 1
  echo @marker-written
}}
set -o pipefail
if ! write_marker 2>/dev/null; then
  if owned "$l"; then
    echo '@marker-warning could not write last-run marker; logs will use a checkout/24h anchor'
    if ! rm -f -- "$m" 2>/dev/null; then
      old=$(head -n 2 "$m" 2>/dev/null | tail -n 1); old=${{old#@run }}
      echo "@marker-warning old marker could not be removed; slip logs may name run $(printf '%s' "$old" | base64 -d 2>/dev/null)"
    fi
  fi
fi
rm -f -- "$tmp" 2>/dev/null || :
"#,
        run = shell_quote(&encode(run_id)),
        plan = shell_quote(&plan),
        sha = shell_quote(sha)
    );
    crate::lock::script(&target.path, run_id, &body)
}

/// Inserted while the lock is owned, before its release move. Every failure
/// becomes a warning; it cannot alter whether the lock is released.
pub(crate) fn finish_body(run_id: &str, outcome: &str) -> String {
    format!(
        r#"if owned "$l"; then
  finish_marker() {{
    m="${{l%/*}}/shipslip.last-run"
    [ -e "$m" ] || return 0
    {{ IFS= read -r version; IFS= read -r run; }} < "$m" || return 1
    [ "$version" = '@marker 1' ] && [ "$run" = {run} ] || return 0
    tmp="$l/marker-end-$$"
    umask 077
    while IFS= read -r line; do
      case "$line" in '@finished '*|'@reflog '*) ;; *) printf '%s\n' "$line" ;; esac
    done < "$m" > "$tmp" || return 1
    printf '@finished %s %s\n' "$(date +%s)" {outcome} >> "$tmp" || return 1
    if tip=$(git reflog -1 --date=unix --format='%H %gD %gs' 2>/dev/null) && [ -n "$tip" ]; then
      printf '@reflog %s\n' "$(printf '%s' "$tip" | base64 -w0)" >> "$tmp" || return 1
    fi
    owned "$l" || return 0
    mv -T -- "$tmp" "$m" || return 1
  }}
  finish_marker 2>/dev/null || echo '@marker-warning could not finish last-run marker; logs may show outcome unknown'
fi
"#,
        run = shell_quote(&format!("@run {}", encode(run_id))),
        outcome = shell_quote(outcome)
    )
}

pub(crate) async fn start<T: Transport>(
    transport: &T,
    target: &DeployTarget,
    run_id: &str,
    plan: RunPlan,
    sha: &str,
) -> Vec<String> {
    let (code, lines) = run_collect(transport, &start_script(target, run_id, plan, sha)).await;
    match code {
        Ok(0) => warnings(&lines),
        _ => vec!["could not write last-run marker; log anchor may be unavailable".into()],
    }
}

pub(crate) fn warnings(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .filter_map(|line| line.strip_prefix("@marker-warning ").map(str::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> String {
        format!("@marker 1\n@run {}\n@plan rerun\n@target {}\n@started 100\n@file 7 300 {} {}\n@end\n@finished 101 succeeded\n@reflog {}\n", encode("r-one"), "a".repeat(40), "b".repeat(64), encode("storage/logs/a'\n.log"), encode("tip"))
    }
    #[test]
    fn marker_roundtrips_paths_and_requires_a_complete_valid_record() {
        let marker = Marker::parse(&sample()).unwrap();
        assert_eq!(marker.run_id, "r-one");
        assert_eq!(marker.files[0].path, "storage/logs/a'\n.log");
        assert_eq!(marker.finished_at, Some(101));
        for invalid in [
            sample().replace("@end\n", ""),
            sample().replace("@marker 1", "@marker 2"),
            sample().replace("101 succeeded", "99 succeeded"),
            sample().replace("@started 100", "@started nope"),
            sample().replace(&encode("r-one"), &encode("../../other")),
        ] {
            assert!(Marker::parse(&invalid).is_none());
        }
        assert!(Marker::parse(&"x".repeat(MAX_MARKER_BYTES + 1)).is_none());
    }
    #[test]
    fn finish_is_guarded_and_does_not_write_another_run() {
        let body = finish_body("r-one", "succeeded");
        assert!(body.starts_with("if owned \"$l\""));
        assert!(body.find("[ \"$run\" =").unwrap() < body.find("mv -T").unwrap());
        assert!(body.contains("|| echo '@marker-warning"));
    }
}
