//! Wording for the environment comparison, outside records.

use super::{EnvComparison, Qualifier};
use crate::git::Comparison;
use crate::logs::escape_field;
use std::fmt::Write;

pub fn comparison_text(a: &str, b: &str, comparison: &EnvComparison) -> String {
    let commits = |n: usize| if n == 1 { "commit" } else { "commits" };
    let mut line = match &comparison.result {
        Comparison::Same => format!("{a} and {b} are on the same commit"),
        Comparison::Ahead(n) => format!("{a} is {n} {} ahead of {b}", commits(*n)),
        Comparison::Behind(n) => format!("{a} is {n} {} behind {b}", commits(*n)),
        Comparison::Diverged { a: left, b: right } => {
            format!("{a} and {b} have diverged: {left} only on {a}, {right} only on {b}")
        }
        Comparison::Unavailable(reason) => format!("cannot compare: {reason}"),
    };
    if !comparison.notes.is_empty() {
        let notes: Vec<String> = comparison
            .notes
            .iter()
            .map(|(env, qualifier)| match qualifier {
                Qualifier::DeployIncomplete => format!("{env} deploy incomplete"),
                Qualifier::TreeDirty => format!("{env} tree dirty"),
            })
            .collect();
        let _ = write!(line, " ({})", notes.join("; "));
    }
    escape_field(&line)
}
