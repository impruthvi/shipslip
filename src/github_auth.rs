//! Temporary GitHub authentication for preflight fetches. Tokens are never
//! included in deploy targets, previews, receipts, or detached step scripts.

use std::fmt;

use base64::Engine;
use zeroize::Zeroizing;

use crate::runner::run_collect;
use crate::script::shell_quote;
use crate::transport::{Transport, TransportError};

/// A local GitHub token supplied explicitly for one preflight fetch.
/// The trusted server receives it in memory over SSH. It is not persisted by
/// Shipslip, and debug formatting never reveals it.
pub struct GitHubToken(Zeroizing<String>);

impl GitHubToken {
    pub fn new(token: String) -> Result<Self, InvalidGitHubToken> {
        let token = Zeroizing::new(token);
        if token.is_empty()
            || token.len() > 4096
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(InvalidGitHubToken);
        }
        Ok(Self(token))
    }

    /// Exposes the token for local identity validation. Do not log or persist
    /// the returned value or pass it in process arguments.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    pub(crate) fn redact(&self, output: &str) -> String {
        let basic = base64::engine::general_purpose::STANDARD
            .encode(format!("x-access-token:{}", self.expose_secret()));
        output
            .replace(&basic, "[REDACTED]")
            .replace(self.expose_secret(), "[REDACTED]")
    }

    /// No secret is passed in argv or written to a file. Only the fetch's
    /// subshell exports it; the helper handles `get` and ignores `store` /
    /// `erase`. Reset helpers for this exact URL to prevent existing helpers
    /// from saving it. Keep authenticated command output off the wire.
    pub(crate) fn fetch_script(&self, repository: &GitHubRepository, refspec: &str) -> String {
        FETCH
            .replace("@TOKEN@", &shell_quote(self.expose_secret()))
            .replace("@REPOSITORY@", &shell_quote(repository.name()))
            .replace("@REFSPEC@", &shell_quote(refspec))
    }
}

impl fmt::Debug for GitHubToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GitHubToken([REDACTED])")
    }
}

#[derive(Debug, thiserror::Error)]
#[error("GitHub token must be nonempty and contain only ASCII letters, digits, and underscores")]
pub struct InvalidGitHubToken;

/// The repository shown to the user before sending credentials to the server.
/// Pinning this value makes a changed origin fail before the authenticated fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubRepository(String);

impl GitHubRepository {
    pub fn from_origin(origin: &str) -> Result<Self, RepositoryError> {
        let path = origin
            .strip_prefix("https://github.com/")
            .map(|s| s.strip_suffix('/').unwrap_or(s))
            .or_else(|| origin.strip_prefix("git@github.com:"))
            .or_else(|| origin.strip_prefix("ssh://git@github.com/"))
            .ok_or(RepositoryError::UnsupportedOrigin)?;
        let path = path.strip_suffix(".git").unwrap_or(path);
        let parts: Vec<_> = path.split('/').collect();
        if parts.len() != 2
            || !parts.iter().all(|part| {
                !part.is_empty()
                    && *part != "."
                    && *part != ".."
                    && part.len() <= 100
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
            })
        {
            return Err(RepositoryError::UnsupportedOrigin);
        }
        Ok(Self(path.to_string()))
    }

    pub fn name(&self) -> &str {
        &self.0
    }

    pub fn owner(&self) -> &str {
        self.0.split_once('/').expect("validated repository").0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    #[error("temporary token authentication requires a standard github.com HTTPS or SSH origin without embedded credentials")]
    UnsupportedOrigin,
    #[error("could not inspect the server's Git origin")]
    InspectFailed,
    #[error(transparent)]
    Transport(#[from] TransportError),
}

/// Inspect origin without sending any credentials or printing its raw URL.
pub async fn discover_repository<T: Transport>(
    transport: &T,
    path: &str,
) -> Result<GitHubRepository, RepositoryError> {
    let script = format!(
        "set +x +v\ncd {} || exit $?\norigin=$(git config --get-all remote.origin.url) || exit $?\ncase \"$origin\" in *$'\\n'*) exit 2 ;; esac\nprintf '@github-origin %s\\n' \"$origin\"\n",
        shell_quote(path)
    );
    let (result, lines) = run_collect(transport, &script).await;
    if result? != 0 {
        return Err(RepositoryError::InspectFailed);
    }
    let origins: Vec<_> = lines
        .iter()
        .filter_map(|line| line.strip_prefix("@github-origin "))
        .collect();
    // Multiple URLs are rejected even when Git's first URL would be usable.
    if origins.len() != 1 {
        return Err(RepositoryError::UnsupportedOrigin);
    }
    GitHubRepository::from_origin(origins[0])
}

const FETCH: &str = r#"set +x +v
origin=$(git config --get-all remote.origin.url 2>/dev/null)
if [[ "$origin" =~ ^https://github\.com/([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)/?$ ]] ||
   [[ "$origin" =~ ^git@github\.com:([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)$ ]] ||
   [[ "$origin" =~ ^ssh://git@github\.com/([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)$ ]]; then
  repo=${BASH_REMATCH[1]}
  repo=${repo%.git}
else
  echo '@fetch_failed'
  echo '@message --github-token requires a standard github.com HTTPS or SSH origin without embedded credentials'
  exit 0
fi
if [ "$repo" != @REPOSITORY@ ]; then
  echo '@fetch_failed'
  echo '@message Git origin changed since account confirmation; run the command again to review the repository'
  exit 0
fi
url="https://github.com/$repo.git"
if [ "$(git ls-remote --get-url "$url" 2>/dev/null)" != "$url" ]; then
  echo '@fetch_failed'
  echo '@message --github-token refuses Git URL rewrites; remove the matching url.insteadOf setting'
  exit 0
fi
if ! fetch_reason=$( (
  set +x +v
  unset GIT_ASKPASS SSH_ASKPASS GIT_CONFIG_PARAMETERS GIT_CONFIG_COUNT GIT_CURL_VERBOSE
  for name in $(compgen -v GIT_TRACE); do unset "$name"; done
  export SHIPSLIP_GITHUB_TOKEN=@TOKEN@
  export SHIPSLIP_GITHUB_REPO="$repo.git"
  helper='!f() {
    set +x +v
    [ "$1" = get ] || exit 0
    protocol= host= path=
    while IFS= read -r line && [ -n "$line" ]; do
      case "$line" in
        protocol=*) protocol=${line#protocol=} ;;
        host=*) host=${line#host=} ;;
        path=*) path=${line#path=} ;;
      esac
    done
    [ "$protocol" = https ] && [ "$host" = github.com ] &&
      [ "$path" = "$SHIPSLIP_GITHUB_REPO" ] || exit 0
    printf "username=x-access-token\npassword=%s\n" "$SHIPSLIP_GITHUB_TOKEN"
  }; f'
  if out=$(git -c credential.helper= \
      -c "credential.$url.helper=" \
      -c "credential.$url.helper=$helper" \
      -c credential.useHttpPath=true \
      -c "credential.$url.useHttpPath=true" \
      -c credential.interactive=false \
      -c core.askPass=/bin/false \
      -c core.hooksPath=/dev/null \
      -c http.followRedirects=false \
      -c "http.$url.followRedirects=false" \
      -c "http.$url.sslVerify=true" \
      -c "http.$url.extraHeader=" \
      fetch --no-recurse-submodules "$url" @REFSPEC@ 2>&1 </dev/null); then
    exit 0
  fi
  case "$out" in
    (*'Could not resolve host'*) reason='The server could not resolve github.com; check server DNS' ;;
    (*'Failed to connect'*|*'Connection timed out'*) reason='The server could not connect to GitHub; check its network or proxy' ;;
    (*'SSL certificate problem'*|*'server certificate verification failed'*) reason='The server could not verify GitHub TLS; check server certificates or proxy' ;;
    (*'Authentication failed'*|*'Invalid username or token'*) reason='GitHub rejected the fetch credentials; check the token and repository read access' ;;
    (*) reason='GitHub fetch failed; check repository access, token expiry, and organization approval or SSO authorization' ;;
  esac
  printf '%s' "$reason"
  exit 1
) 2>/dev/null); then
  echo '@fetch_failed'
  printf '@message %s\n' "$fetch_reason"
  exit 0
fi
unset origin repo url fetch_reason
"#;

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn repository_parsing_rejects_credentials_and_other_hosts() {
        for origin in [
            "https://github.com/acme/app.git",
            "https://github.com/acme/app/",
            "git@github.com:acme/app.git",
            "ssh://git@github.com/acme/app",
        ] {
            assert_eq!(
                GitHubRepository::from_origin(origin).unwrap().name(),
                "acme/app"
            );
        }
        for origin in [
            "https://user:secret@github.com/acme/app.git",
            "https://github.com.evil/acme/app",
            "http://github.com/acme/app",
            "https://github.com/acme/app?token=secret",
            "https://github.com/acme/app#fragment",
            "git@other:acme/app.git",
            "ssh://github.com/acme/app",
            "https://github.com/acme/../app",
            "https://github.com/acme/..",
            "https://github.com/acme/.git",
            "https://github.com/acme/app\nhttps://github.com/other/app",
        ] {
            assert!(GitHubRepository::from_origin(origin).is_err(), "{origin}");
        }
    }

    #[test]
    fn tokens_validate_without_disclosing_input_and_redact_basic_auth() {
        let token = GitHubToken::new("github_pat_TEST_123".into()).unwrap();
        assert_eq!(format!("{token:?}"), "GitHubToken([REDACTED])");
        let basic =
            base64::engine::general_purpose::STANDARD.encode("x-access-token:github_pat_TEST_123");
        assert_eq!(
            token.redact(&format!("github_pat_TEST_123 {basic}")),
            "[REDACTED] [REDACTED]"
        );
        for bad in [
            "",
            "a\nb",
            "token with spaces",
            "$(touch file)",
            "secret\"value",
        ] {
            let error = GitHubToken::new(bad.into()).unwrap_err().to_string();
            assert!(!error.contains("secret\"value"));
        }
    }

    struct Fixture(PathBuf);

    impl Fixture {
        fn new(origin: &str) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "shipslip-auth-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let fixture = Self(dir);
            fixture.git(&["init", "-q"]);
            fixture.git(&["remote", "add", "origin", origin]);
            // Both generic and URL-specific pre-existing helpers must be reset.
            let spy = format!(
                "!echo called >> {}; cat >> {}",
                shell_quote(&fixture.0.join("helper-spy").to_string_lossy()),
                shell_quote(&fixture.0.join("helper-spy").to_string_lossy())
            );
            fixture.git(&["config", "credential.helper", &spy]);
            fixture.git(&[
                "config",
                "credential.https://github.com/acme/app.git.helper",
                &spy,
            ]);
            fixture.git(&[
                "config",
                "credential.https://github.com.useHttpPath",
                "false",
            ]);
            fixture
        }

        fn command(&self, binary: &str) -> Command {
            let mut cmd = Command::new(binary);
            cmd.current_dir(&self.0)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env_remove("GIT_CONFIG_PARAMETERS")
                .env_remove("GIT_CONFIG_COUNT")
                .env_remove("GIT_ASKPASS")
                .env_remove("SSH_ASKPASS");
            cmd
        }

        fn git(&self, args: &[&str]) -> String {
            let output = self.command("git").args(args).output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        }

        fn fetch(&self, fail: bool) -> String {
            let token = GitHubToken::new("github_pat_TEST_123".into()).unwrap();
            let repo = GitHubRepository::from_origin("https://github.com/acme/app.git").unwrap();
            // Real Git resolves the credential configuration and invokes the
            // helper. Replace only network fetch, checking its credential
            // context, approve/erase behavior, and isolation from later steps.
            let script = format!(
                "{}\n{}\n[ -z \"${{SHIPSLIP_GITHUB_TOKEN+x}}\" ] || exit 93\necho @auth-ok\n",
                MOCK_FETCH,
                token.fetch_script(&repo, "+refs/heads/main:refs/remotes/origin/main")
            );
            let mut child = self
                .command("bash")
                .args(["-xv", "-s"])
                .env("FAIL_FETCH", if fail { "1" } else { "0" })
                .env("GIT_TERMINAL_PROMPT", "0")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(script.as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let output = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!output.contains(token.expose_secret()), "secret leaked");
            assert!(!self.0.join("helper-spy").exists(), "existing helper ran");
            assert!(!self.0.join("trace").exists(), "trace file was written");
            assert!(!self
                .git(&["config", "--list"])
                .contains(token.expose_secret()));
            output
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const MOCK_FETCH: &str = r#"set +x +v
git() {
  if [ "$1" != -c ]; then command git "$@"; return; fi
  configs=()
  while [ "$1" = -c ]; do configs+=("$1" "$2"); shift 2; done
  [ "$1" = fetch ] && [ "$2" = --no-recurse-submodules ] &&
    [ "$3" = https://github.com/acme/app.git ] || return 80
  [ -z "${GIT_TRACE_CURL+x}" ] || return 81
  [ "$(command git "${configs[@]}" config --get-urlmatch http.followRedirects "$3")" = false ] || return 82
  [ "$(command git "${configs[@]}" config --get core.hooksPath)" = /dev/null ] || return 83
  credential=$(printf 'protocol=https\nhost=github.com\npath=acme/app.git\n\n' |
    command git "${configs[@]}" credential fill) || return 84
  [[ "$credential" == *"password=$SHIPSLIP_GITHUB_TOKEN"* ]] || return 85
  for context in 'protocol=http\nhost=github.com\npath=acme/app.git' \
                 'protocol=https\nhost=evil.example\npath=acme/app.git' \
                 'protocol=https\nhost=github.com\npath=acme/other.git'; do
    if printf '%b\n\n' "$context" | command git "${configs[@]}" credential fill; then return 86; fi
  done
  printf '%s\n\n' "$credential" | command git "${configs[@]}" credential approve || return 87
  printf '%s\n\n' "$credential" | command git "${configs[@]}" credential reject || return 88
  if [ "$FAIL_FETCH" = 1 ]; then echo "Authentication failed $SHIPSLIP_GITHUB_TOKEN"; return 1; fi
}
export GIT_TRACE_CURL="$PWD/trace"
"#;

    #[test]
    fn credential_helper_is_scoped_ephemeral_and_ignores_existing_stores() {
        for origin in [
            "https://github.com/acme/app.git",
            "git@github.com:acme/app.git",
            "ssh://git@github.com/acme/app",
        ] {
            let fixture = Fixture::new(origin);
            let before = fixture.git(&["config", "--get", "remote.origin.url"]);
            let output = fixture.fetch(false);
            assert!(output.contains("@auth-ok"), "{output}");
            assert!(!output.contains("@fetch_failed"), "{output}");
            assert_eq!(
                fixture.git(&["config", "--get", "remote.origin.url"]),
                before
            );
        }
    }

    #[test]
    fn failed_fetch_hides_output_and_rejects_changed_origins_and_rewrites() {
        let fixture = Fixture::new("https://github.com/acme/app.git");
        let output = fixture.fetch(true);
        assert!(output.contains("@fetch_failed"));
        assert!(
            output.contains("GitHub rejected the fetch credentials"),
            "{output}"
        );
        assert!(!output.contains("@auth-ok"));
        fixture.git(&[
            "remote",
            "set-url",
            "origin",
            "https://github.com/other/app.git",
        ]);
        assert!(fixture.fetch(false).contains("Git origin changed"));
        fixture.git(&[
            "remote",
            "set-url",
            "origin",
            "https://github.com/acme/app.git",
        ]);
        fixture.git(&[
            "config",
            "url.https://evil.example/.insteadOf",
            "https://github.com/",
        ]);
        assert!(fixture.fetch(false).contains("refuses Git URL rewrites"));
    }
}
