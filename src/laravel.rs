use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Installer compatibility floor exercised by `scripts/installer-matrix.sh`.
pub const MIN_INSTALLER_VERSION: &str = "5.31.1";

#[derive(Debug, Error)]
pub enum LaravelError {
    #[error("{0}")]
    Invalid(String),
    #[error("Laravel installer is incompatible: {0}; upgrade Laravel Installer to {MIN_INSTALLER_VERSION} or newer")]
    Incompatible(String),
    #[error("invalid scaffold: {0}")]
    Scaffold(String),
    #[error("could not read {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
}

macro_rules! choices {
    ($name:ident, $flag:literal, {$($variant:ident => $text:literal),+ $(,)?}) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "lowercase")]
        pub enum $name { $($variant),+ }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(match self { $(Self::$variant => $text),+ })
            }
        }
        impl FromStr for $name {
            type Err = LaravelError;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value {
                    $($text => Ok(Self::$variant)),+,
                    _ => Err(LaravelError::Invalid(format!("{} must be one of: {}", $flag, [$($text),+].join(", ")))),
                }
            }
        }
    };
}

choices!(StarterKit, "--starter-kit", { None => "none", React => "react", Vue => "vue", Svelte => "svelte", Livewire => "livewire" });
choices!(Database, "--database", { Sqlite => "sqlite", Mysql => "mysql", Mariadb => "mariadb", Pgsql => "pgsql", Sqlsrv => "sqlsrv" });
choices!(Testing, "--testing", { Pest => "pest", Phpunit => "phpunit" });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Auth {
    Laravel,
    None,
}

impl fmt::Display for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Laravel => "laravel",
            Self::None => "none",
        })
    }
}

impl FromStr for Auth {
    type Err = LaravelError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "laravel" => Ok(Self::Laravel),
            "none" => Ok(Self::None),
            "workos" => Err(LaravelError::Invalid(
                "WorkOS authentication is not yet supported".into(),
            )),
            _ => Err(LaravelError::Invalid(
                "--auth must be laravel or none".into(),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallerOptions {
    pub starter_kit: StarterKit,
    pub auth: Auth,
    pub database: Database,
    pub testing: Testing,
    pub boost: bool,
}

impl InstallerOptions {
    pub fn validate(&self) -> Result<(), LaravelError> {
        if self.starter_kit == StarterKit::None && self.auth != Auth::None {
            return Err(LaravelError::Invalid(
                "--starter-kit none requires --auth none".into(),
            ));
        }
        Ok(())
    }

    pub fn args(&self, destination: &Path) -> Vec<OsString> {
        let mut args = vec!["new".into(), destination.as_os_str().into(), "-n".into()];
        if self.starter_kit != StarterKit::None {
            args.push(format!("--{}", self.starter_kit).into());
        }
        if self.auth == Auth::None {
            args.push("--no-authentication".into());
        }
        args.push(format!("--database={}", self.database).into());
        args.push(format!("--{}", self.testing).into());
        args.push("--npm".into());
        args.push(if self.boost { "--boost" } else { "--no-boost" }.into());
        args
    }

    pub fn probe(&self, version: &str, help: &str) -> Result<String, LaravelError> {
        self.validate()?;
        let version = version
            .split_whitespace()
            .find(|word| word.as_bytes().first().is_some_and(u8::is_ascii_digit))
            .ok_or_else(|| {
                LaravelError::Incompatible("could not parse installer version".into())
            })?;
        let parsed = parse_version(version)?;
        if parsed < parse_version(MIN_INSTALLER_VERSION)? {
            return Err(LaravelError::Incompatible(format!(
                "installed version {version} is too old"
            )));
        }
        let flags: Vec<_> = help
            .split_whitespace()
            .map(|word| word.split(['=', '[']).next().unwrap_or(word))
            .collect();
        let mut required = vec![
            "--database".to_string(),
            "--no-interaction".into(),
            "--npm".into(),
            format!("--{}", self.testing),
            if self.boost {
                "--boost".into()
            } else {
                "--no-boost".into()
            },
        ];
        if self.starter_kit != StarterKit::None {
            required.push(format!("--{}", self.starter_kit));
        }
        if self.auth == Auth::None {
            required.push("--no-authentication".into());
        }
        for flag in required {
            if !flags.contains(&flag.as_str()) {
                return Err(LaravelError::Incompatible(format!("missing {flag}")));
            }
        }
        Ok(version.into())
    }
}

pub(crate) fn parse_version(version: &str) -> Result<(u32, u32, u32), LaravelError> {
    let parts: Vec<_> = version.split('.').collect();
    if parts.len() != 3 {
        return Err(LaravelError::Incompatible(
            "unrecognized installer version".into(),
        ));
    }
    let number = |part: &str| {
        part.parse::<u32>()
            .map_err(|_| LaravelError::Incompatible("unrecognized installer version".into()))
    };
    Ok((number(parts[0])?, number(parts[1])?, number(parts[2])?))
}

#[derive(Debug, Clone)]
pub struct ScaffoldInfo {
    pub framework_version: String,
}

fn read_json(path: &Path) -> Result<Value, LaravelError> {
    let bytes = fs::read(path).map_err(|source| LaravelError::Io {
        path: path.into(),
        source,
    })?;
    serde_json::from_slice(&bytes)
        .map_err(|_| LaravelError::Scaffold(format!("{} is not valid JSON", path.display())))
}

pub fn validate_scaffold(
    root: &Path,
    options: &InstallerOptions,
) -> Result<ScaffoldInfo, LaravelError> {
    options.validate()?;
    if !fs::symlink_metadata(root)
        .map_err(|source| LaravelError::Io {
            path: root.into(),
            source,
        })?
        .file_type()
        .is_dir()
    {
        return Err(LaravelError::Scaffold(
            "scaffold must be a real directory, not a symlink".into(),
        ));
    }
    if fs::symlink_metadata(root.join(".git")).is_ok() {
        return Err(LaravelError::Scaffold(
            "installer created .git; ShipSlip must initialize Git itself".into(),
        ));
    }
    for path in [
        "artisan",
        "composer.lock",
        "vendor/autoload.php",
        "package-lock.json",
        "public/build/manifest.json",
    ] {
        if !root.join(path).is_file() {
            return Err(LaravelError::Scaffold(format!("missing {path}")));
        }
    }
    let composer = read_json(&root.join("composer.json"))?;
    let lock = read_json(&root.join("composer.lock"))?;
    let npm = read_json(&root.join("package.json"))?;
    read_json(&root.join("package-lock.json"))?;
    let framework_version = lock["packages"]
        .as_array()
        .and_then(|packages| {
            packages
                .iter()
                .find(|package| package["name"] == "laravel/framework")
        })
        .and_then(|package| package["version"].as_str())
        .ok_or_else(|| {
            LaravelError::Scaffold("composer.lock has no Laravel framework version".into())
        })?
        .to_string();
    let testing_package = match options.testing {
        Testing::Pest => "pestphp/pest",
        Testing::Phpunit => "phpunit/phpunit",
    };
    let locked = |name: &str| {
        ["packages", "packages-dev"].iter().any(|group| {
            lock[*group]
                .as_array()
                .is_some_and(|packages| packages.iter().any(|package| package["name"] == name))
        })
    };
    if options.boost && !locked("laravel/boost") {
        return Err(LaravelError::Scaffold(
            "composer.lock has no requested Laravel Boost package laravel/boost".into(),
        ));
    }
    if !locked(testing_package) {
        return Err(LaravelError::Scaffold(format!(
            "composer.lock has no selected testing package {testing_package}"
        )));
    }
    if npm["scripts"]["build"]
        .as_str()
        .is_none_or(|script| script.trim().is_empty())
    {
        return Err(LaravelError::Scaffold(
            "package.json has no build script".into(),
        ));
    }
    let has = |package: &str| {
        [
            &composer["require"],
            &composer["require-dev"],
            &npm["dependencies"],
            &npm["devDependencies"],
        ]
        .iter()
        .any(|packages| packages.get(package).is_some())
    };
    match options.starter_kit {
        StarterKit::None => {
            if composer["name"] != "laravel/laravel"
                || [
                    "react",
                    "react-dom",
                    "vue",
                    "svelte",
                    "livewire/livewire",
                    "laravel/fortify",
                    "inertiajs/inertia-laravel",
                    "@inertiajs/react",
                    "@inertiajs/vue3",
                    "@inertiajs/svelte",
                ]
                .iter()
                .any(|package| has(package))
            {
                return Err(LaravelError::Scaffold(
                    "plain Laravel unexpectedly contains a starter kit".into(),
                ));
            }
        }
        kit => {
            let marker = match kit {
                StarterKit::React => "@inertiajs/react",
                StarterKit::Vue => "@inertiajs/vue3",
                StarterKit::Svelte => "@inertiajs/svelte",
                StarterKit::Livewire => "livewire/livewire",
                StarterKit::None => unreachable!(),
            };
            if !has(marker) {
                return Err(LaravelError::Scaffold(format!(
                    "missing kit marker {marker}"
                )));
            }
            if has("laravel/fortify") != (options.auth == Auth::Laravel) {
                return Err(LaravelError::Scaffold(
                    "authentication packages do not match requested auth".into(),
                ));
            }
        }
    }
    let manifest = read_json(&root.join("public/build/manifest.json"))?;
    let entries = manifest
        .as_object()
        .filter(|entries| !entries.is_empty())
        .ok_or_else(|| LaravelError::Scaffold("empty or invalid build manifest".into()))?;
    for entry in entries.values() {
        let asset = entry["file"]
            .as_str()
            .ok_or_else(|| LaravelError::Scaffold("manifest entry has no asset".into()))?;
        for asset in std::iter::once(asset).chain(["css", "assets"].into_iter().flat_map(|key| {
            entry[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
        })) {
            let path = Path::new(asset);
            if path.is_absolute()
                || path
                    .components()
                    .any(|component| !matches!(component, std::path::Component::Normal(_)))
                || !root.join("public/build").join(path).is_file()
            {
                return Err(LaravelError::Scaffold(format!(
                    "missing or unsafe build asset {asset}"
                )));
            }
        }
    }
    Ok(ScaffoldInfo { framework_version })
}

#[cfg(test)]
#[path = "laravel_tests.rs"]
mod scaffold_tests;

#[cfg(test)]
mod tests {
    use super::*;
    const HELP: &str = include_str!("fixtures/laravel-installer-5.31.1-help.txt");
    fn options(kit: StarterKit, auth: Auth) -> InstallerOptions {
        InstallerOptions {
            starter_kit: kit,
            auth,
            database: Database::Sqlite,
            testing: Testing::Pest,
            boost: false,
        }
    }
    #[test]
    fn combination_table_and_golden_argv() {
        for kit in [
            StarterKit::None,
            StarterKit::React,
            StarterKit::Vue,
            StarterKit::Svelte,
            StarterKit::Livewire,
        ] {
            for auth in [Auth::None, Auth::Laravel] {
                let options = options(kit, auth);
                if kit == StarterKit::None && auth == Auth::Laravel {
                    assert!(options.validate().is_err());
                    continue;
                }
                options.validate().unwrap();
                let mut expected = vec!["new".to_string(), "my app".into(), "-n".into()];
                if kit != StarterKit::None {
                    expected.push(format!("--{kit}"));
                }
                if auth == Auth::None {
                    expected.push("--no-authentication".into());
                }
                expected.extend(
                    ["--database=sqlite", "--pest", "--npm", "--no-boost"].map(str::to_string),
                );
                assert_eq!(
                    options.args(Path::new("my app")),
                    expected.iter().map(OsString::from).collect::<Vec<_>>()
                );
                assert_eq!(
                    options.probe("Laravel Installer 5.31.1\n", HELP).unwrap(),
                    "5.31.1"
                );
            }
        }
        assert!("workos"
            .parse::<Auth>()
            .unwrap_err()
            .to_string()
            .contains("not yet supported"));
    }
    #[test]
    fn required_capabilities_and_minimum_version() {
        let options = options(StarterKit::None, Auth::None);
        assert!(options
            .probe("Laravel Installer 5.30.0", HELP)
            .unwrap_err()
            .to_string()
            .contains("upgrade"));
        assert!(options
            .probe(
                "Laravel Installer 5.31.1",
                &HELP.replace("--no-boost", "--other-option")
            )
            .unwrap_err()
            .to_string()
            .contains("--no-boost"));
        assert!(options
            .probe(
                "Laravel Installer 5.31.1",
                &HELP.replace("--npm", "--npm-future")
            )
            .is_err());
        for version in ["unknown", "5.31", "5.31.1-alpha"] {
            assert!(options.probe(version, HELP).is_err());
        }
    }
    #[test]
    fn all_database_testing_and_boost_flags_are_explicit() {
        for db in ["sqlite", "mysql", "mariadb", "pgsql", "sqlsrv"] {
            for test in ["pest", "phpunit"] {
                let mut options = options(StarterKit::React, Auth::Laravel);
                options.database = db.parse().unwrap();
                options.testing = test.parse().unwrap();
                options.boost = true;
                let args = options.args(Path::new("app"));
                assert!(args.contains(&format!("--database={db}").into()));
                assert!(args.contains(&format!("--{test}").into()));
                assert!(args.contains(&"--boost".into()));
                options.probe("Laravel Installer 5.31.1", HELP).unwrap();
            }
        }
    }
}
