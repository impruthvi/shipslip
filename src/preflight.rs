//! Read-only checks of the app's git checkout on the server.
//!
//! Scripts report facts as `@key value` lines, so output from login shell
//! startup files is ignored, and the decisions are made here.

use std::fmt;

use crate::script::{shell_quote, wrap_step};

/// Why preflight refused to deploy. Nothing on the server was changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockReason {
    /// `origin/<branch>` is the commit already checked out.
    UpToDate,
    /// Uncommitted changes, as `git status --porcelain` lines.
    DirtyTree(Vec<String>),
    /// A merge, rebase, cherry-pick or revert is in progress.
    OperationInProgress(String),
    /// `actual` is empty when HEAD is detached.
    WrongBranch {
        expected: String,
        actual: String,
    },
    FetchFailed(String),
    /// `origin/<branch>` does not match what was just fetched.
    FetchMismatch,
    /// A rerun requires the requested release to be checked out already.
    RerunTargetMismatch {
        head: String,
        target: String,
    },
    /// The checked-out commit is not an ancestor of `origin/<branch>`.
    NotFastForward,
    /// `bash -n` rejected a step script.
    InvalidStep {
        step: usize,
        message: String,
    },
    CommandFailed {
        code: i32,
        output: String,
    },
}

impl fmt::Display for BlockReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UpToDate => write!(f, "already up to date"),
            Self::DirtyTree(files) => write!(f, "uncommitted changes: {}", files.join(", ")),
            Self::OperationInProgress(op) => write!(f, "git operation in progress ({op})"),
            Self::WrongBranch { expected, actual } if actual.is_empty() => {
                write!(f, "HEAD is detached, expected branch `{expected}`")
            }
            Self::WrongBranch { expected, actual } => {
                write!(f, "on branch `{actual}`, expected `{expected}`")
            }
            Self::FetchFailed(output) => {
                write!(f, "the server couldn't fetch from origin: {output}")
            }
            Self::FetchMismatch => write!(f, "fetched commit does not match origin"),
            Self::RerunTargetMismatch { head, target } => write!(
                f,
                "cannot rerun: checkout is {head}, but origin points to {target}"
            ),
            Self::NotFastForward => write!(f, "origin is not a fast-forward of the server's HEAD"),
            Self::InvalidStep { step, message } => write!(f, "step {step} is invalid: {message}"),
            Self::CommandFailed { code, output } => {
                write!(f, "preflight exited with {code}: {output}")
            }
        }
    }
}

/// Why a deploy stopped at the recheck, before changing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbortReason {
    LockLost,
    /// HEAD is no longer the commit the preview was built from.
    HeadMoved {
        expected: String,
        actual: String,
    },
    DirtyTree(Vec<String>),
    OperationInProgress(String),
    /// `php artisan down` exited with this code.
    MaintenanceDownFailed(i32),
    ConnectFailed(String),
    CheckFailed(String),
}

impl fmt::Display for AbortReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LockLost => write!(f, "the deploy lock was taken over"),
            Self::HeadMoved { expected, actual } => {
                write!(f, "HEAD moved from {expected} to {actual}")
            }
            Self::DirtyTree(files) => write!(f, "uncommitted changes: {}", files.join(", ")),
            Self::OperationInProgress(op) => write!(f, "git operation in progress ({op})"),
            Self::MaintenanceDownFailed(code) => {
                write!(f, "`php artisan down` exited with {code}")
            }
            Self::ConnectFailed(reason) => write!(f, "could not reach the server: {reason}"),
            Self::CheckFailed(reason) => write!(f, "could not check the server: {reason}"),
        }
    }
}

/// The checkout's state: HEAD, branch, operations in progress, changes.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct State {
    pub head: String,
    pub branch: String,
    pub operation: Option<String>,
    pub dirty: Vec<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Preflight {
    pub state: State,
    pub fetch_failed: Option<String>,
    pub target: String,
    pub fetch_head: String,
    pub ancestor: bool,
    pub commits: Vec<String>,
    pub invalid_step: Option<(usize, String)>,
}

pub(crate) fn state_script(path: &str) -> String {
    format!(
        r#"cd {path} || exit $?
g=$(git rev-parse --absolute-git-dir) || exit $?
echo "@head $(git rev-parse HEAD)"
echo "@branch $(git symbolic-ref --quiet --short HEAD)"
for op in MERGE_HEAD rebase-merge rebase-apply CHERRY_PICK_HEAD REVERT_HEAD; do
  [ -e "$g/$op" ] && echo "@operation $op"
done
git status --porcelain | sed 's/^/@dirty /'
"#,
        path = shell_quote(path),
    )
}

/// State, then fetch, ancestry and the commit list, then `bash -n` of each
/// recipe step's script (`steps[0]` is step 1).
pub(crate) fn preflight_script(path: &str, branch: &str, steps: &[String]) -> String {
    let mut s = state_script(path);
    s.push_str(&format!(
        r#"export GIT_TERMINAL_PROMPT=0 SSH_ASKPASS_REQUIRE=never
if ! out=$(git fetch origin {refspec} 2>&1 </dev/null); then
  echo "@fetch_failed"
  printf '%s\n' "$out" | sed 's/^/@message /'
  exit 0
fi
h=$(git rev-parse HEAD)
t=$(git rev-parse {remote})
echo "@target $t"
echo "@fetch_head $(git rev-parse FETCH_HEAD)"
git merge-base --is-ancestor "$h" "$t" && echo "@ancestor"
git log --oneline "$h..$t" | sed 's/^/@commit /'
"#,
        refspec = shell_quote(&format!(
            "+refs/heads/{branch}:refs/remotes/origin/{branch}"
        )),
        remote = shell_quote(&format!("origin/{branch}")),
    ));
    for (n, step) in (1..).zip(steps) {
        s.push_str(&format!(
            "if ! out=$(printf '%s' {} | bash -n 2>&1); then\n  \
             echo \"@invalid {n}\"\n  \
             printf '%s\\n' \"$out\" | sed 's/^/@message /'\n\
             fi\n",
            shell_quote(&wrap_step(path, step)),
        ));
    }
    s
}

/// `(key, value)` for each `@key value` line.
fn facts(lines: &[String]) -> impl Iterator<Item = (&str, &str)> {
    lines.iter().filter_map(|l| {
        let l = l.strip_prefix('@')?;
        Some(l.split_once(' ').unwrap_or((l, "")))
    })
}

pub(crate) fn parse_state(lines: &[String]) -> State {
    let mut state = State::default();
    for (key, value) in facts(lines) {
        match key {
            "head" => state.head = value.to_string(),
            "branch" => state.branch = value.to_string(),
            "operation" if state.operation.is_none() => state.operation = Some(value.into()),
            "dirty" => state.dirty.push(value.to_string()),
            _ => {}
        }
    }
    state
}

pub(crate) fn parse_preflight(lines: &[String]) -> Preflight {
    let mut p = Preflight {
        state: parse_state(lines),
        ..Preflight::default()
    };
    let mut messages = Vec::new();
    for (key, value) in facts(lines) {
        match key {
            "fetch_failed" => p.fetch_failed = Some(String::new()),
            "target" => p.target = value.to_string(),
            "fetch_head" => p.fetch_head = value.to_string(),
            "ancestor" => p.ancestor = true,
            "commit" => p.commits.push(value.to_string()),
            "invalid" if p.invalid_step.is_none() => {
                p.invalid_step = value.parse().ok().map(|n| (n, String::new()))
            }
            "message" if p.fetch_failed.is_some() || p.invalid_step.is_some() => {
                messages.push(value)
            }
            _ => {}
        }
    }
    let message = messages.join("\n");
    if let Some(output) = &mut p.fetch_failed {
        *output = message;
    } else if let Some((_, text)) = &mut p.invalid_step {
        *text = message;
    }
    p
}

/// The first reason to refuse, in the order a person would fix them.
pub(crate) fn block_reason(
    p: &Preflight,
    branch: &str,
    allow_up_to_date: bool,
) -> Option<BlockReason> {
    let s = &p.state;
    if let Some(op) = &s.operation {
        return Some(BlockReason::OperationInProgress(op.clone()));
    }
    if !s.dirty.is_empty() {
        return Some(BlockReason::DirtyTree(s.dirty.clone()));
    }
    if s.branch != branch {
        return Some(BlockReason::WrongBranch {
            expected: branch.into(),
            actual: s.branch.clone(),
        });
    }
    if let Some(output) = &p.fetch_failed {
        return Some(BlockReason::FetchFailed(output.clone()));
    }
    if p.target != p.fetch_head {
        return Some(BlockReason::FetchMismatch);
    }
    if p.target == s.head && !allow_up_to_date {
        return Some(BlockReason::UpToDate);
    }
    if !p.ancestor {
        return Some(BlockReason::NotFastForward);
    }
    if allow_up_to_date && p.target != s.head {
        return Some(BlockReason::RerunTargetMismatch {
            head: s.head.clone(),
            target: p.target.clone(),
        });
    }
    p.invalid_step
        .as_ref()
        .map(|(step, message)| BlockReason::InvalidStep {
            step: *step,
            message: message.clone(),
        })
}

/// Why the checkout is no longer safe to change, if it isn't.
pub(crate) fn recheck(state: &State, from_sha: &str) -> Option<AbortReason> {
    if let Some(op) = &state.operation {
        return Some(AbortReason::OperationInProgress(op.clone()));
    }
    if state.head != from_sha {
        return Some(AbortReason::HeadMoved {
            expected: from_sha.into(),
            actual: state.head.clone(),
        });
    }
    if !state.dirty.is_empty() {
        return Some(AbortReason::DirtyTree(state.dirty.clone()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn clean() -> Preflight {
        Preflight {
            state: State {
                head: "a".into(),
                branch: "main".into(),
                ..State::default()
            },
            target: "b".into(),
            fetch_head: "b".into(),
            ancestor: true,
            ..Preflight::default()
        }
    }

    #[test]
    fn preflight_output_is_parsed() {
        let p = parse_preflight(&lines(&[
            "motd from .profile",
            "@head a",
            "@branch main",
            "@dirty  M file",
            "@dirty ?? new",
            "@target b",
            "@fetch_head b",
            "@ancestor",
            "@commit b2 Second",
            "@invalid 1",
            "@message bash: line 7: unexpected EOF",
            "@message bash: syntax error",
        ]));
        assert_eq!(p.state.head, "a");
        assert_eq!(p.state.dirty, [" M file", "?? new"]);
        assert!(p.ancestor);
        assert_eq!(p.commits, ["b2 Second"]);
        assert_eq!(
            p.invalid_step,
            Some((1, "bash: line 7: unexpected EOF\nbash: syntax error".into()))
        );
    }

    #[test]
    fn fetch_failure_keeps_git_output() {
        let p = parse_preflight(&lines(&[
            "@head a",
            "@branch main",
            "@fetch_failed",
            "@message Permission denied (publickey).",
        ]));
        assert_eq!(
            p.fetch_failed.as_deref(),
            Some("Permission denied (publickey).")
        );
    }

    #[test]
    fn block_reasons_in_order() {
        assert_eq!(block_reason(&clean(), "main", false), None);

        let mut p = clean();
        p.state.operation = Some("MERGE_HEAD".into());
        p.state.dirty = vec![" M f".into()];
        assert_eq!(
            block_reason(&p, "main", false),
            Some(BlockReason::OperationInProgress("MERGE_HEAD".into()))
        );
        p.state.operation = None;
        assert!(matches!(
            block_reason(&p, "main", false),
            Some(BlockReason::DirtyTree(_))
        ));

        let mut p = clean();
        assert_eq!(
            block_reason(&p, "release", false),
            Some(BlockReason::WrongBranch {
                expected: "release".into(),
                actual: "main".into()
            })
        );
        p.fetch_head = "c".into();
        assert_eq!(
            block_reason(&p, "main", false),
            Some(BlockReason::FetchMismatch)
        );

        let mut p = clean();
        p.target = "a".into();
        p.fetch_head = "a".into();
        assert_eq!(block_reason(&p, "main", false), Some(BlockReason::UpToDate));
        assert_eq!(block_reason(&p, "main", true), None);

        let mut p = clean();
        p.ancestor = false;
        assert_eq!(
            block_reason(&p, "main", false),
            Some(BlockReason::NotFastForward)
        );

        let mut p = clean();
        p.invalid_step = Some((2, "oops".into()));
        assert_eq!(
            block_reason(&p, "main", false),
            Some(BlockReason::InvalidStep {
                step: 2,
                message: "oops".into()
            })
        );
    }

    #[test]
    fn rerun_requires_the_fetched_target_to_be_checked_out() {
        let mut p = clean();
        p.state.head = "a".into();
        p.target = "b".into();
        p.fetch_head = "b".into();
        assert_eq!(
            block_reason(&p, "main", true),
            Some(BlockReason::RerunTargetMismatch {
                head: "a".into(),
                target: "b".into(),
            })
        );
    }

    #[test]
    fn recheck_catches_changes_since_preview() {
        let state = |head: &str, dirty: &[&str]| State {
            head: head.into(),
            branch: "main".into(),
            operation: None,
            dirty: dirty.iter().map(|s| s.to_string()).collect(),
        };
        assert_eq!(recheck(&state("a", &[]), "a"), None);
        assert!(matches!(
            recheck(&state("b", &[]), "a"),
            Some(AbortReason::HeadMoved { .. })
        ));
        assert!(matches!(
            recheck(&state("a", &[" M f"]), "a"),
            Some(AbortReason::DirtyTree(_))
        ));
    }

    #[test]
    fn every_step_is_syntax_checked_as_generated() {
        let steps = vec!["true".to_string(), "echo 'hi'".to_string()];
        let s = preflight_script("/app", "main", &steps);
        assert!(s.contains("echo \"@invalid 1\""));
        assert!(s.contains("echo \"@invalid 2\""));
        assert!(s.contains(&shell_quote(&wrap_step("/app", "echo 'hi'"))));
    }
}
