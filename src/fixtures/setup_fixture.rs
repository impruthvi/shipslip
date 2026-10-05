use super::setup_api::{
    self, DetectionContext, DetectionOptions, DetectionReport, Purpose, SetupInputs,
};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct Fixture {
    pub root: PathBuf,
    pub context: DetectionContext,
}

pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub const SCRUBBED: &str = r#"
for key in GH_TOKEN GITHUB_TOKEN GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN SHIPSLIP_GITHUB_TOKEN; do
    eval 'value=${'"$key"'+present}'
    [ -z "$value" ] || exit 81
done
if read -r unexpected; then exit 82; fi
"#;

impl Fixture {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "shipslip-repair-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        for directory in ["bin", "home/.composer", "brew/bin", "sources", "system"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        let context = DetectionContext {
            environment: BTreeMap::from([
                ("HOME".into(), root.join("home").into_os_string()),
                (
                    "COMPOSER_HOME".into(),
                    root.join("home/.composer").into_os_string(),
                ),
                ("PATH".into(), root.join("bin").into_os_string()),
            ]),
            macos: true,
            system_bin: root.join("system"),
            xdg_system: root.join("xdg"),
            herd_app: root.join("Herd.app"),
            brew_candidates: vec![root.join("brew/bin/brew")],
        };
        let fixture = Self { root, context };
        fixture.script(&fixture.root.join("system/xcode-select"), "exit 0");
        fixture.sources("vendor/bin");
        fixture.brew(&format!(
            r#"
case "$*" in
  --prefix) echo {} ;;
  install*)
    {SCRUBBED}
    [ "$HOMEBREW_NO_AUTO_UPDATE" = 1 ]
    [ "$HOMEBREW_NO_INSTALL_UPGRADE" = 1 ]
    [ "$HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK" = 1 ]
    [ "$HOMEBREW_NO_INSTALL_CLEANUP" = 1 ]
    echo "$*" >> {}/actions
    shift
    for formula do
      /bin/cp {}/sources/"$formula" {}/brew/bin/"$formula"
      /bin/chmod 700 {}/brew/bin/"$formula"
      if [ "$formula" = node ]; then /bin/cp {}/sources/npm {}/brew/bin/npm; fi
    done ;;
  *) exit 99 ;;
esac
"#,
            quote(&fixture.root.join("brew").to_string_lossy()),
            quote(&fixture.root.to_string_lossy()),
            quote(&fixture.root.to_string_lossy()),
            quote(&fixture.root.to_string_lossy()),
            quote(&fixture.root.to_string_lossy()),
            quote(&fixture.root.to_string_lossy()),
            quote(&fixture.root.to_string_lossy())
        ));
        fixture
    }

    pub fn script(&self, path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Keep writable executable descriptors out of the multithreaded test process.
        let mut writer = Command::new("/bin/sh")
            .args([
                "-c",
                "umask 077; /bin/cat > \"$1\" && /bin/chmod 700 \"$1\"",
                "writer",
            ])
            .arg(path)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        writer
            .stdin
            .take()
            .unwrap()
            .write_all(format!("#!/bin/sh\nset -eu\n{body}\n").as_bytes())
            .unwrap();
        assert!(writer.wait().unwrap().success());
    }

    pub fn brew(&self, body: &str) {
        self.script(&self.context.brew_candidates[0], body);
    }

    pub fn sources(&self, bin: &str) {
        let root = quote(&self.root.to_string_lossy());
        self.script(
            &self.root.join("sources/php"),
            &format!(
                r#"
case "$1" in
  --version) echo 'PHP 8.4.0' ;;
  -m) printf '%s\n' {} ;;
  *composer) echo "$0" > {root}/interpreter; exec /bin/sh "$@" ;;
  *) exit 99 ;;
esac
"#,
                quote(&setup_api::PHP_EXTENSIONS.join("\n"))
            ),
        );
        let composer = format!(
            r#"#!/usr/bin/env php
set -eu
case "$*" in
  --version) echo 'Composer 2.8.0' ;;
  'global require laravel/installer')
    {SCRUBBED}
    echo composer >> {root}/actions
    /bin/mkdir -p {root}/home/.composer/{bin}
    /bin/cp {root}/sources/laravel {root}/home/.composer/{bin}/laravel ;;
  *) exit 99 ;;
esac
"#
        );
        self.script(&self.root.join("sources/composer"), "exit 99");
        // The shebang is intentional: the executor must bind the approved PHP.
        let mut writer = Command::new("/bin/sh")
            .args([
                "-c",
                "umask 077; /bin/cat > \"$1\" && /bin/chmod 700 \"$1\"",
                "writer",
            ])
            .arg(self.root.join("sources/composer"))
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        writer
            .stdin
            .take()
            .unwrap()
            .write_all(composer.as_bytes())
            .unwrap();
        assert!(writer.wait().unwrap().success());
        self.script(&self.root.join("sources/laravel"), &format!("if [ \"$1\" = --version ]; then echo 'Laravel Installer 5.31.1'; else printf '%s\\n' {}; fi", quote(include_str!("laravel-installer-5.31.1-help.txt"))));
        self.script(&self.root.join("sources/node"), "echo v22.0.0");
        self.script(&self.root.join("sources/npm"), "echo 10.0.0");
        self.script(&self.root.join("sources/gh"), "echo 'gh 2.0.0'");
        self.script(
            &self.root.join("sources/git"),
            &format!(
                r#"
if [ "$1" = -c ]; then shift 2; fi
case "$*" in
  --version) echo 'git 2.0.0' ;;
  'config --get user.name') [ -f {root}/name ] && /bin/cat {root}/name ;;
  'config --get user.email') [ -f {root}/email ] && /bin/cat {root}/email ;;
  'config --global user.name '*) {SCRUBBED}
    printf '%s\n' "$4" > {root}/name; echo name >> {root}/actions ;;
  'config --global user.email '*) {SCRUBBED}
    printf '%s\n' "$4" > {root}/email; echo email >> {root}/actions ;;
  *) exit 99 ;;
esac
"#
            ),
        );
    }

    pub fn install(&self, name: &str) {
        self.copy(
            &self.root.join("sources").join(name),
            &self.root.join("brew/bin").join(name),
        );
    }

    pub fn copy(&self, source: &Path, target: &Path) {
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        assert!(Command::new("/bin/cp")
            .args([source, target])
            .status()
            .unwrap()
            .success());
    }

    pub fn complete(&self) {
        for name in ["php", "composer", "node", "npm", "git", "gh"] {
            self.install(name);
        }
        fs::create_dir_all(self.root.join("home/.composer/vendor/bin")).unwrap();
        self.copy(
            &self.root.join("sources/laravel"),
            &self.root.join("home/.composer/vendor/bin/laravel"),
        );
        fs::write(self.root.join("name"), "Tester\n").unwrap();
        fs::write(self.root.join("email"), "tester@example.com\n").unwrap();
    }

    pub fn options(&self) -> DetectionOptions {
        DetectionOptions {
            purposes: vec![Purpose::Create],
            root: self.root.clone(),
            installer_options: None,
        }
    }

    pub async fn detect(&self) -> DetectionReport {
        setup_api::detect(&self.context, &self.options())
            .await
            .unwrap()
    }

    pub fn inputs(&self) -> SetupInputs {
        SetupInputs {
            name: Some("Tester".into()),
            email: Some("tester@example.com".into()),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
