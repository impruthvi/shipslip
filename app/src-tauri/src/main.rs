#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

mod commands;
#[cfg(test)]
mod fixture;
mod git_path;
mod receipt_view;
mod state;
mod view;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(Arc::new(commands::App::from_env()))
        .invoke_handler(tauri::generate_handler![
            commands::load_map,
            commands::compare,
            commands::load_receipt
        ])
        .run(tauri::generate_context!())
        .expect("error while running Shipslip");
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    fn json(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn capability_grants_only_window_event_and_clipboard_write() {
        let cap = json(include_str!("../capabilities/default.json"));
        let mut perms: Vec<&str> = cap["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().expect("permission ids only, no scoped objects"))
            .collect();
        perms.sort();
        assert_eq!(
            perms,
            [
                "clipboard-manager:allow-write-text",
                "core:event:default",
                "core:window:default",
            ]
        );
        assert_eq!(cap["windows"], json(r#"["main"]"#));
    }

    #[test]
    fn only_capability_file_is_default() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/capabilities");
        let names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|name| {
                [".json", ".json5", ".toml"]
                    .iter()
                    .any(|ext| name.ends_with(ext))
            })
            .collect();
        assert_eq!(names, ["default.json"]);
    }

    #[test]
    fn identifier_does_not_end_with_app() {
        let conf = json(include_str!("../tauri.conf.json"));
        let id = conf["identifier"].as_str().unwrap();
        assert!(
            !id.ends_with(".app"),
            "{id} clashes with the macOS bundle extension"
        );
    }

    #[test]
    fn window_is_light_with_minimum_size() {
        let conf = json(include_str!("../tauri.conf.json"));
        let window = &conf["app"]["windows"][0];
        assert_eq!(window["label"], "main");
        assert_eq!(window["theme"], "Light");
        assert_eq!(window["minWidth"], 1024);
        assert_eq!(window["minHeight"], 680);
    }

    fn sources(csp: &str) -> Vec<&str> {
        csp.split(';')
            .flat_map(|directive| directive.split_whitespace().skip(1))
            .collect()
    }

    #[test]
    fn csp_allows_only_local_sources() {
        let conf = json(include_str!("../tauri.conf.json"));
        let security = &conf["app"]["security"];
        let csp = security["csp"].as_str().unwrap();
        assert!(csp.contains("default-src 'self'"));
        for src in sources(csp) {
            assert!(
                ["'self'", "'none'", "ipc:", "http://ipc.localhost"].contains(&src),
                "csp allows {src}"
            );
        }
        let dev = security["devCsp"].as_str().unwrap();
        for src in sources(dev) {
            assert!(
                [
                    "'self'",
                    "'none'",
                    "'unsafe-inline'",
                    "ipc:",
                    "http://ipc.localhost",
                    "ws://localhost:1420",
                ]
                .contains(&src),
                "devCsp allows {src}"
            );
        }
        assert!(security
            .get("dangerousDisableAssetCspModification")
            .is_none());
        assert!(conf["app"].get("withGlobalTauri").is_none());
    }
}
