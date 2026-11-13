use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

/// A temp `.ssh` folder, removed on drop.
struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "shipslip-ssh-aliases-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn list_leaves_out_wildcard_and_negated_patterns() {
    let fx = Fixture::new();
    let config = fx.write(
        "config",
        "# work\nHost *\n  User me\n\nHost web1 web2 *.internal !bastion\n  HostName 203.0.113.7\n\
         Host=db?\nhost  \"crm\"   # quoted\nMatch host web1\n  User deploy\nHost web1\n",
    );

    assert_eq!(list(&config).unwrap(), ["web1", "web2", "crm"]);
}

#[test]
fn list_follows_include_relative_to_the_config_folder_and_by_glob() {
    let fx = Fixture::new();
    let config = fx.write(
        "config",
        "Include config.d/*.conf\nHost local\nInclude ~/never/there\n",
    );
    fx.write("config.d/b.conf", "Host second\n");
    fx.write("config.d/a.conf", "Host first\nInclude nested\n");
    fx.write("config.d/skip.txt", "Host skipped\n");
    fx.write("nested", "Host deep\n");

    assert_eq!(list(&config).unwrap(), ["first", "deep", "second", "local"]);
}

#[test]
fn an_include_loop_stops() {
    let fx = Fixture::new();
    let config = fx.write("config", "Host top\nInclude config\n");

    assert_eq!(list(&config).unwrap(), ["top"]);
}

#[test]
fn a_missing_config_lists_nothing() {
    let fx = Fixture::new();
    assert_eq!(list(&fx.0.join("config")).unwrap(), Vec::<String>::new());
}

#[test]
fn listing_never_writes_the_config() {
    let fx = Fixture::new();
    let text = "Host web1\n  HostName 203.0.113.7\n";
    let config = fx.write("config", text);
    fs::set_permissions(&config, fs::Permissions::from_mode(0o400)).unwrap();
    let before = fs::metadata(&config).unwrap().modified().unwrap();

    list(&config).unwrap();

    assert_eq!(fs::read_to_string(&config).unwrap(), text);
    assert_eq!(fs::metadata(&config).unwrap().modified().unwrap(), before);
    assert_eq!(
        fs::metadata(&config).unwrap().permissions().mode() & 0o777,
        0o400
    );
    let names: Vec<_> = fs::read_dir(&fx.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["config"]);
}

fn draft() -> HostDraft {
    HostDraft {
        alias: "crm".into(),
        host_name: "203.0.113.7".into(),
        user: "forge".into(),
        identity_file: "~/.ssh/id_ed25519".into(),
        port: "22".into(),
    }
}

#[test]
fn the_host_block_leaves_out_port_22() {
    assert_eq!(
        draft().block().unwrap(),
        "Host crm\n  HostName 203.0.113.7\n  User forge\n  IdentityFile ~/.ssh/id_ed25519\n"
    );
    let mut empty_port = draft();
    empty_port.port = " ".into();
    assert_eq!(empty_port.block().unwrap(), draft().block().unwrap());
}

#[test]
fn the_host_block_names_another_port_and_leaves_out_empty_fields() {
    let mut other = draft();
    other.port = "2222".into();
    other.user = String::new();
    other.identity_file = String::new();

    assert_eq!(
        other.block().unwrap(),
        "Host crm\n  HostName 203.0.113.7\n  Port 2222\n"
    );
}

#[test]
fn the_host_block_refuses_values_that_would_add_lines() {
    let bad = HostDraft {
        alias: "crm *".into(),
        host_name: "".into(),
        user: "forge\n  ProxyCommand sh".into(),
        identity_file: "/etc/passwd".into(),
        port: "70000".into(),
    };

    let problems: Vec<HostField> = bad
        .block()
        .unwrap_err()
        .into_iter()
        .map(|(field, _)| field)
        .collect();
    assert_eq!(
        problems,
        [
            HostField::Alias,
            HostField::HostName,
            HostField::User,
            HostField::IdentityFile,
            HostField::Port
        ]
    );
}

#[test]
fn identity_files_are_the_private_keys_named_id() {
    let fx = Fixture::new();
    for name in [
        "id_rsa",
        "id_rsa.pub",
        "id_ed25519",
        "id_ed25519.pub",
        "config",
        "known_hosts",
        "id_ed25519_work",
    ] {
        fx.write(name, "");
    }
    fs::create_dir_all(fx.0.join("id_dir")).unwrap();

    assert_eq!(
        identity_files(&fx.0),
        [
            "~/.ssh/id_ed25519",
            "~/.ssh/id_ed25519_work",
            "~/.ssh/id_rsa"
        ]
    );
    assert_eq!(identity_files(&fx.0.join("missing")), Vec::<String>::new());
}

#[test]
fn resolve_reads_ssh_g_for_the_alias() {
    let fx = Fixture::new();
    let config = fx.write(
        "config",
        "Host crm\n  HostName 203.0.113.7\n  User forge\n  Port 2222\n",
    );

    let resolved = resolve("crm", Some(&config)).unwrap();
    assert_eq!(
        resolved,
        Resolved {
            host_name: "203.0.113.7".into(),
            user: "forge".into(),
            port: 2222,
        }
    );
}

#[test]
fn resolve_refuses_a_name_that_is_not_an_alias() {
    assert!(matches!(
        resolve("-oProxyCommand=sh", None),
        Err(ResolveError::NotAnAlias)
    ));
}

#[test]
fn a_draft_matches_only_what_ssh_resolves_to() {
    let fx = Fixture::new();
    let config = fx.write("config", "Host other\n  HostName 198.51.100.1\n");
    let missing = resolve("crm", Some(&config)).unwrap();
    assert!(!draft().matches(&missing));

    let config = fx.write("config", "Host crm\n  HostName 203.0.113.7\n  User forge\n");
    let found = resolve("crm", Some(&config)).unwrap();
    assert!(draft().matches(&found));

    let mut other_user = draft();
    other_user.user = "deploy".into();
    assert!(!other_user.matches(&found));
    let mut any_user = draft();
    any_user.user = String::new();
    assert!(any_user.matches(&found));
    let mut other_port = draft();
    other_port.port = "2222".into();
    assert!(!other_port.matches(&found));
}
