//! Regression fixtures for the observable scaffold validation contract.
use super::*;
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct Scaffold(PathBuf);
impl Scaffold {
    fn new(kit: StarterKit, auth: Auth) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let fixture = Self(std::env::temp_dir().join(format!(
            "shipslip-scaffold-validation-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        fs::create_dir_all(&fixture.0).unwrap();
        for (file, text) in [
            ("artisan", "<?php // app entrypoint"),
            ("vendor/autoload.php", "<?php // Composer autoload"),
            ("public/build/assets/app-123.js", "console.log('app')"),
            ("public/build/assets/app-123.css", "body { color: black }"),
            ("public/build/assets/logo-123.svg", "<svg/>"),
        ] {
            fixture.write(file, text);
        }
        let mut composer =
            json!({"name":"laravel/laravel", "require":{"laravel/framework":"^13.0"}});
        let mut npm = json!({"scripts":{"build":"vite build"},"devDependencies":{"vite":"^7.0"}});
        match kit {
            StarterKit::None => {}
            StarterKit::React => {
                composer["name"] = json!("laravel/react-starter-kit");
                npm["dependencies"] = json!({"@inertiajs/react":"^3.0","react":"^19.0"});
            }
            StarterKit::Vue => {
                composer["name"] = json!("laravel/vue-starter-kit");
                npm["dependencies"] = json!({"@inertiajs/vue3":"^3.0","vue":"^3.5"});
            }
            StarterKit::Svelte => {
                composer["name"] = json!("laravel/svelte-starter-kit");
                npm["dependencies"] = json!({"@inertiajs/svelte":"^3.0","svelte":"^5.0"});
            }
            StarterKit::Livewire => {
                composer["name"] = json!("laravel/livewire-starter-kit");
                composer["require"]["livewire/livewire"] = json!("^4.0");
            }
        }
        if auth == Auth::Laravel {
            composer["require"]["laravel/fortify"] = json!("^1.0");
        }
        fixture.json("composer.json", composer);
        fixture.json(
            "composer.lock",
            json!({"packages":[{"name":"laravel/framework","version":"v13.34.0"}],"packages-dev":[{"name":"pestphp/pest","version":"v4.0.0"},{"name":"phpunit/phpunit","version":"12.0.0"}]}),
        );
        fixture.json("package.json", npm);
        fixture.json(
            "package-lock.json",
            json!({"lockfileVersion":3,"packages":{}}),
        );
        fixture.json(
            "public/build/manifest.json",
            json!({"resources/js/app.js":{"file":"assets/app-123.js","css":["assets/app-123.css"],"assets":["assets/logo-123.svg"]}}),
        );
        fixture
    }
    fn write(&self, path: &str, value: &str) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }
    fn json(&self, path: &str, value: Value) {
        self.write(path, &serde_json::to_string(&value).unwrap());
    }
    fn rejected(&self, options: &InstallerOptions) -> String {
        validate_scaffold(&self.0, options)
            .expect_err("unsafe or incomplete scaffold accepted")
            .to_string()
    }
}
impl Drop for Scaffold {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
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
fn all_nine_supported_scaffolds_validate_and_report_framework_version() {
    let mut tested = 0;
    for kit in [
        StarterKit::None,
        StarterKit::React,
        StarterKit::Vue,
        StarterKit::Svelte,
        StarterKit::Livewire,
    ] {
        for auth in [Auth::None, Auth::Laravel] {
            if kit == StarterKit::None && auth == Auth::Laravel {
                continue;
            }
            let fixture = Scaffold::new(kit, auth);
            let info = validate_scaffold(&fixture.0, &options(kit, auth)).unwrap();
            assert_eq!(info.framework_version, "v13.34.0");
            tested += 1;
        }
    }
    assert_eq!(tested, 9);
}

#[test]
fn incomplete_installation_or_missing_built_assets_cannot_be_verified() {
    for missing in [
        "artisan",
        "composer.lock",
        "vendor/autoload.php",
        "package-lock.json",
        "package.json",
        "public/build/manifest.json",
        "public/build/assets/app-123.js",
        "public/build/assets/app-123.css",
        "public/build/assets/logo-123.svg",
    ] {
        let fixture = Scaffold::new(StarterKit::None, Auth::None);
        fs::remove_file(fixture.0.join(missing)).unwrap();
        assert!(
            fixture
                .rejected(&options(StarterKit::None, Auth::None))
                .contains(missing.rsplit('/').next().unwrap()),
            "missing {missing} was not identified"
        );
    }
    let fixture = Scaffold::new(StarterKit::None, Auth::None);
    fixture.json("package.json", json!({"scripts":{"dev":"vite"}}));
    assert!(fixture
        .rejected(&options(StarterKit::None, Auth::None))
        .contains("build script"));
}

#[test]
fn plain_laravel_must_have_plain_root_name_and_no_accidental_kit_packages() {
    let fixture = Scaffold::new(StarterKit::React, Auth::None);
    assert!(fixture
        .rejected(&options(StarterKit::None, Auth::None))
        .contains("starter kit"));
    let fixture = Scaffold::new(StarterKit::None, Auth::None);
    fixture.json(
        "composer.json",
        json!({"name":"someone/custom-app", "require":{"laravel/framework":"^13.0"}}),
    );
    assert!(fixture
        .rejected(&options(StarterKit::None, Auth::None))
        .contains("starter kit"));
    for accidental in ["react", "vue", "svelte", "@inertiajs/react"] {
        let fixture = Scaffold::new(StarterKit::None, Auth::None);
        let mut npm = read_json(&fixture.0.join("package.json")).unwrap();
        npm["dependencies"] = json!({accidental:"latest"});
        fixture.json("package.json", npm);
        assert!(fixture
            .rejected(&options(StarterKit::None, Auth::None))
            .contains("starter kit"));
    }
}

#[test]
fn actual_kit_and_authentication_must_match_requested_choices() {
    let fixture = Scaffold::new(StarterKit::React, Auth::Laravel);
    assert!(fixture
        .rejected(&options(StarterKit::Vue, Auth::Laravel))
        .contains("kit marker"));
    assert!(fixture
        .rejected(&options(StarterKit::React, Auth::None))
        .contains("authentication"));
    let fixture = Scaffold::new(StarterKit::Livewire, Auth::None);
    assert!(fixture
        .rejected(&options(StarterKit::Livewire, Auth::Laravel))
        .contains("authentication"));
}

#[test]
fn malformed_json_and_framework_lock_without_a_version_are_rejected() {
    for path in [
        "composer.json",
        "composer.lock",
        "package.json",
        "package-lock.json",
        "public/build/manifest.json",
    ] {
        let fixture = Scaffold::new(StarterKit::None, Auth::None);
        fixture.write(path, "{ truncated");
        let error = fixture.rejected(&options(StarterKit::None, Auth::None));
        assert!(error.contains("not valid JSON") && error.contains(path));
    }
    let fixture = Scaffold::new(StarterKit::None, Auth::None);
    fixture.json(
        "composer.lock",
        json!({"packages":[{"name":"laravel/framework"}]}),
    );
    assert!(fixture
        .rejected(&options(StarterKit::None, Auth::None))
        .contains("framework version"));
    for malformed in [json!({}), json!([]), json!({"app":{}})] {
        let fixture = Scaffold::new(StarterKit::None, Auth::None);
        fixture.json("public/build/manifest.json", malformed);
        fixture.rejected(&options(StarterKit::None, Auth::None));
    }
}

#[test]
fn manifest_assets_cannot_escape_build_directory() {
    let fixture = Scaffold::new(StarterKit::None, Auth::None);
    fixture.write("public/escape.js", "outside build directory");
    let absolute = fixture
        .0
        .join("public/escape.js")
        .to_string_lossy()
        .into_owned();
    for path in ["../escape.js", "assets/../../escape.js", &absolute] {
        for field in ["file", "css", "assets"] {
            let mut manifest = json!({"app":{"file":"assets/app-123.js"}});
            manifest["app"][field] = if field == "file" {
                json!(path)
            } else {
                json!([path])
            };
            fixture.json("public/build/manifest.json", manifest);
            assert!(fixture
                .rejected(&options(StarterKit::None, Auth::None))
                .contains("unsafe build asset"));
        }
    }
}

#[test]
fn installer_git_repository_is_rejected_even_if_it_is_a_file() {
    for is_dir in [false, true] {
        let fixture = Scaffold::new(StarterKit::None, Auth::None);
        if is_dir {
            fs::create_dir(fixture.0.join(".git")).unwrap();
        } else {
            fixture.write(".git", "gitdir: somewhere");
        }
        assert!(fixture
            .rejected(&options(StarterKit::None, Auth::None))
            .contains("installer created .git"));
    }
}

#[cfg(unix)]
#[test]
fn broken_git_symlink_and_symlink_scaffold_root_are_rejected() {
    use std::os::unix::fs::symlink;
    let fixture = Scaffold::new(StarterKit::None, Auth::None);
    symlink(fixture.0.join("nonexistent"), fixture.0.join(".git")).unwrap();
    assert!(fixture
        .rejected(&options(StarterKit::None, Auth::None))
        .contains("installer created .git"));
    fs::remove_file(fixture.0.join(".git")).unwrap();
    let link = fixture.0.join("aliased-root");
    symlink(&fixture.0, &link).unwrap();
    assert!(
        validate_scaffold(&link, &options(StarterKit::None, Auth::None))
            .unwrap_err()
            .to_string()
            .contains("real directory")
    );
}

#[test]
fn selected_test_runner_must_be_installed_in_composer_lock() {
    let fixture = Scaffold::new(StarterKit::None, Auth::None);
    let mut request = options(StarterKit::None, Auth::None);
    let mut lock = read_json(&fixture.0.join("composer.lock")).unwrap();
    lock["packages-dev"] = json!([{"name":"phpunit/phpunit","version":"12.0.0"}]);
    fixture.json("composer.lock", lock.clone());
    assert!(fixture.rejected(&request).contains("pestphp/pest"));
    request.testing = Testing::Phpunit;
    validate_scaffold(&fixture.0, &request).unwrap();
    lock["packages-dev"] = json!([]);
    fixture.json("composer.lock", lock);
    assert!(fixture.rejected(&request).contains("phpunit/phpunit"));
}
#[test]
fn invalid_plain_auth_request_is_rejected_before_scaffold_validation() {
    let fixture = Scaffold::new(StarterKit::None, Auth::None);
    assert!(fixture
        .rejected(&options(StarterKit::None, Auth::Laravel))
        .contains("--starter-kit none requires --auth none"));
}

#[test]
fn requested_boost_must_be_installed_before_verifying_scaffold() {
    let fixture = Scaffold::new(StarterKit::None, Auth::None);
    let mut request = options(StarterKit::None, Auth::None);
    validate_scaffold(&fixture.0, &request).unwrap();
    request.boost = true;
    assert!(fixture.rejected(&request).contains("laravel/boost"));
    let mut lock = read_json(&fixture.0.join("composer.lock")).unwrap();
    lock["packages-dev"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"laravel/boost","version":"v2.0.0"}));
    fixture.json("composer.lock", lock);
    validate_scaffold(&fixture.0, &request).unwrap();
}
