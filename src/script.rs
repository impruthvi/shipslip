//! Generation of the bash scripts sent to the server.

/// Quotes `s` as a single-quoted bash word.
pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Wraps a step so it runs in the app directory, in strict mode, without
/// stdin, with stderr merged into stdout so output keeps its order.
///
/// The step text is inserted verbatim: it is user-authored shell and is
/// treated as arbitrary execution.
pub(crate) fn wrap_step(path: &str, body: &str) -> String {
    format!(
        "exec 2>&1\n\
         cd {} || exit $?\n\
         export GIT_TERMINAL_PROMPT=0 SSH_ASKPASS_REQUIRE=never\n\
         (\n\
         set -eo pipefail\n\
         {}\n\
         ) </dev/null\n",
        shell_quote(path),
        body
    )
}

/// True for branch names that are safe to put in a git refspec unquoted.
pub(crate) fn is_safe_branch(branch: &str) -> bool {
    !branch.is_empty()
        && !branch.starts_with('-')
        && !branch.contains("..")
        && branch
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
}

/// True for a full git object id (SHA-1 or SHA-256).
pub(crate) fn is_full_sha(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_single_quotes() {
        assert_eq!(shell_quote("/var/www/it's"), r"'/var/www/it'\''s'");
    }

    #[test]
    fn wrapper_is_strict_and_has_no_stdin() {
        let s = wrap_step("/var/www/app", "php artisan migrate --force");
        assert!(s.starts_with("exec 2>&1\ncd '/var/www/app' || exit $?\n"));
        assert!(s.contains("set -eo pipefail\nphp artisan migrate --force\n) </dev/null"));
    }

    #[test]
    fn step_text_is_inserted_verbatim() {
        let body = r#"echo "$HOME" 'x' `date`; true"#;
        assert!(wrap_step("/a", body).contains(body));
    }

    #[test]
    fn branch_validation() {
        assert!(is_safe_branch("main"));
        assert!(is_safe_branch("release/1.2_x"));
        assert!(!is_safe_branch(""));
        assert!(!is_safe_branch("-x"));
        assert!(!is_safe_branch("a..b"));
        assert!(!is_safe_branch("main;rm"));
        assert!(!is_safe_branch("a b"));
    }

    #[test]
    fn sha_validation() {
        assert!(is_full_sha(&"a".repeat(40)));
        assert!(is_full_sha(&"0".repeat(64)));
        assert!(!is_full_sha("abc123"));
        assert!(!is_full_sha(&"g".repeat(40)));
    }
}
