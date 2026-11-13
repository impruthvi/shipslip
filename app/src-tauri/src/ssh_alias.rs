//! The Add project sheet's ssh alias step: the aliases in `~/.ssh/config`,
//! the `Host` block for "None of these fit", and Check again. Nothing here
//! writes an ssh file (ADR 0003).

use std::path::Path;

use serde::Serialize;
use shipslip::logs::escape_field;
use shipslip::ssh_aliases::{self, HostDraft, HostField, ResolveError, Resolved};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshAliases {
    pub aliases: Vec<AliasLine>,
    /// `~/.ssh/id_*` private keys, for the IdentityFile field.
    pub identity_files: Vec<String>,
    /// `~/.ssh/config` could not be read.
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AliasLine {
    pub alias: String,
    /// What `ssh -G` resolves it to; `None` when ssh could not say.
    pub user: Option<String>,
    pub host_name: Option<String>,
}

pub fn aliases(
    config: &Path,
    ssh_dir: &Path,
    resolve: impl Fn(&str) -> Result<Resolved, ResolveError>,
) -> SshAliases {
    let (names, error) = match ssh_aliases::list(config) {
        Ok(names) => (names, None),
        Err(error) => (
            Vec::new(),
            Some(escape_field(&format!(
                "Couldn't read {}: {error}",
                config.display()
            ))),
        ),
    };
    let aliases = names
        .into_iter()
        .map(|alias| {
            let resolved = resolve(&alias).ok();
            AliasLine {
                alias: escape_field(&alias),
                user: resolved.as_ref().map(|r| escape_field(&r.user)),
                host_name: resolved.map(|r| escape_field(&r.host_name)),
            }
        })
        .collect();
    SshAliases {
        aliases,
        identity_files: ssh_aliases::identity_files(ssh_dir),
        error,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostProblem {
    pub field: HostField,
    pub message: String,
}

fn problems(problems: Vec<(HostField, &'static str)>) -> Vec<HostProblem> {
    problems
        .into_iter()
        .map(|(field, message)| HostProblem {
            field,
            message: message.into(),
        })
        .collect()
}

/// The `Host` block for the "None of these fit" fields.
#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum HostBlock {
    /// The block, one entry per line.
    Block {
        lines: Vec<String>,
    },
    Invalid {
        problems: Vec<HostProblem>,
    },
}

pub fn host_block(draft: &HostDraft) -> HostBlock {
    match draft.block() {
        Ok(text) => HostBlock::Block {
            lines: text.lines().map(escape_field).collect(),
        },
        Err(found) => HostBlock::Invalid {
            problems: problems(found),
        },
    }
}

/// What ssh resolves the alias to now, as `ssh -G` prints it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedView {
    pub host_name: String,
    pub user: String,
    pub port: u16,
}

/// Check again: whether `ssh -G <alias>` now shows the fields.
#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AliasCheck {
    Invalid { problems: Vec<HostProblem> },
    Matches { resolved: ResolvedView },
    Differs { resolved: ResolvedView },
    Failed { message: String },
}

/// `listed` is the aliases `~/.ssh/config` names. An alias no `Host` names
/// resolves to itself, so one spelled like its HostName matches only once
/// it is listed: before the paste it would match anyway.
pub fn check(
    draft: &HostDraft,
    listed: &[String],
    resolve: impl Fn(&str) -> Result<Resolved, ResolveError>,
) -> AliasCheck {
    if let Err(found) = draft.block() {
        return AliasCheck::Invalid {
            problems: problems(found),
        };
    }
    match resolve(draft.alias.trim()) {
        Ok(resolved) => {
            let alias = draft.alias.trim();
            let matches = draft.matches(&resolved)
                && (!alias.eq_ignore_ascii_case(draft.host_name.trim())
                    || listed.iter().any(|name| name == alias));
            let resolved = ResolvedView {
                host_name: escape_field(&resolved.host_name),
                user: escape_field(&resolved.user),
                port: resolved.port,
            };
            if matches {
                AliasCheck::Matches { resolved }
            } else {
                AliasCheck::Differs { resolved }
            }
        }
        Err(error) => AliasCheck::Failed {
            message: escape_field(&error.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "shipslip-ssh-alias-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn resolved(host_name: &str, user: &str, port: u16) -> Resolved {
        Resolved {
            host_name: host_name.into(),
            user: user.into(),
            port,
        }
    }

    fn draft() -> HostDraft {
        HostDraft {
            alias: "crm".into(),
            host_name: "203.0.113.7".into(),
            user: "forge".into(),
            identity_file: String::new(),
            port: String::new(),
        }
    }

    #[test]
    fn aliases_show_what_ssh_resolves_each_to_and_the_keys() {
        let fx = Fixture::new();
        fs::write(fx.0.join("config"), "Host *\nHost web1 broken\n").unwrap();
        fs::write(fx.0.join("id_ed25519"), "").unwrap();

        let view = aliases(&fx.0.join("config"), &fx.0, |alias| match alias {
            "web1" => Ok(resolved("203.0.113.10", "deploy", 22)),
            _ => Err(ResolveError::Failed("bad".into())),
        });

        assert_eq!(
            serde_json::to_value(view).unwrap(),
            serde_json::json!({
                "aliases": [
                    {"alias": "web1", "user": "deploy", "hostName": "203.0.113.10"},
                    {"alias": "broken", "user": null, "hostName": null},
                ],
                "identityFiles": ["~/.ssh/id_ed25519"],
                "error": null,
            })
        );
    }

    #[test]
    fn an_unreadable_config_says_so() {
        let fx = Fixture::new();
        fs::create_dir_all(fx.0.join("config")).unwrap();

        let view = aliases(&fx.0.join("config"), &fx.0, |_| unreachable!());
        assert!(view.aliases.is_empty());
        assert!(view.error.unwrap().starts_with("Couldn't read"));
    }

    #[test]
    fn the_block_or_the_fields_to_fix() {
        assert_eq!(
            serde_json::to_value(host_block(&draft())).unwrap(),
            serde_json::json!({"kind": "block", "lines": ["Host crm", "  HostName 203.0.113.7", "  User forge"]})
        );
        let mut bad = draft();
        bad.port = "x".into();
        assert_eq!(
            serde_json::to_value(host_block(&bad)).unwrap(),
            serde_json::json!({"kind": "invalid", "problems": [{"field": "port", "message": "Use a port from 1 to 65535."}]})
        );
    }

    #[test]
    fn check_again_says_whether_ssh_now_shows_the_fields() {
        let json = |check| serde_json::to_value(check).unwrap();
        let found = check(&draft(), &[], |_| Ok(resolved("203.0.113.7", "forge", 22)));
        assert_eq!(json(found)["kind"], "matches");

        let missing = json(check(&draft(), &[], |alias| Ok(resolved(alias, "me", 22))));
        assert_eq!(
            missing,
            serde_json::json!({"kind": "differs", "resolved": {"hostName": "crm", "user": "me", "port": 22}})
        );

        let failed = check(&draft(), &[], |_| {
            Err(ResolveError::Failed("bad config".into()))
        });
        assert_eq!(json(failed)["kind"], "failed");

        let mut bad = draft();
        bad.host_name = String::new();
        assert_eq!(
            json(check(&bad, &[], |_| unreachable!()))["kind"],
            "invalid"
        );
    }

    #[test]
    fn an_alias_spelled_like_its_host_name_matches_only_once_listed() {
        let mut ip = draft();
        ip.alias = "203.0.113.7".into();
        let itself = |alias: &str| Ok(resolved(alias, "forge", 22));

        let before = serde_json::to_value(check(&ip, &[], itself)).unwrap();
        assert_eq!(before["kind"], "differs");
        let listed = ["203.0.113.7".to_string()];
        let after = serde_json::to_value(check(&ip, &listed, itself)).unwrap();
        assert_eq!(after["kind"], "matches");
    }
}
