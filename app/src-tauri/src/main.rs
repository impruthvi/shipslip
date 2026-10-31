#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
use tauri::menu::{Menu, MenuItem, MenuItemKind};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};

mod awake;
mod commands;
mod deploy_session;
mod finish;
#[cfg(test)]
mod fixture;
mod git_path;
mod receipt_view;
mod recovery;
mod state;
mod terminate;
#[cfg(test)]
mod ui_fixtures;
mod view;

const QUIT: &str = "quit";

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .manage(Arc::new(commands::App::from_env()))
        .menu(menu)
        .on_menu_event(|app, event| {
            if event.id() == QUIT {
                request_quit(app);
            }
        })
        .setup(|app| {
            terminate::install(app.handle());
            let handle = app.handle().clone();
            let state = app.state::<Arc<commands::App>>();
            state.on_live_change(move |live| {
                let _ = handle.run_on_main_thread(move || awake::set(live));
            });
            let handle = app.handle().clone();
            state.on_finish(move |ending| {
                let app = handle.clone();
                let _ = handle.run_on_main_thread(move || finish::finished(&app, &ending));
            });
            Ok(())
        })
        .on_page_load(|webview, payload| {
            if webview.label() == "main"
                && payload.event() == tauri::webview::PageLoadEvent::Started
            {
                webview.state::<Arc<commands::App>>().invalidate_page();
            }
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            match event {
                // While a run is live, closing hides the window; the run keeps going.
                WindowEvent::CloseRequested { api, .. } if live(window.app_handle()) => {
                    api.prevent_close();
                    let _ = window.hide();
                }
                WindowEvent::Focused(true) => {
                    let _ = window.set_title(finish::TITLE);
                }
                WindowEvent::Destroyed => {
                    window.state::<Arc<commands::App>>().invalidate_page();
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::load_map,
            commands::compare,
            commands::load_receipt,
            commands::deploy_entry,
            commands::open_config,
            commands::prepare_deploy,
            commands::review_trust,
            commands::approve_trust,
            commands::confirm_deploy,
            commands::cancel_deploy,
            commands::review_up,
            commands::review_attach,
            commands::current_runs,
            commands::live_runs,
            commands::subscribe_run,
            commands::ack_run,
            commands::unsubscribe_run,
            commands::stop_run,
            commands::cancel_pending,
            commands::receipt_seen,
            commands::quit_status,
            commands::detach_and_quit,
            commands::quit_now
        ])
        .build(tauri::generate_context!())
        .expect("error while building Shipslip");
    app.run(|app, event| match event {
        // Dock Quit and other system requests; our own exits carry a code.
        RunEvent::ExitRequested {
            code: None, api, ..
        } if live(app) => {
            api.prevent_exit();
            request_quit(app);
        }
        // Closing the last window or a system quit with a preview open:
        // release its server lock first.
        RunEvent::ExitRequested {
            code: None, api, ..
        } if app.state::<Arc<commands::App>>().previews_open() => {
            api.prevent_exit();
            commands::exit(app);
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => show_main(app),
        _ => {}
    });
}

fn live(app: &AppHandle) -> bool {
    !app.state::<Arc<commands::App>>().quit_runs().is_empty()
}

/// A run is live or a preview holds a server lock: quitting goes through
/// `request_quit` first.
fn quit_needs_app(app: &AppHandle) -> bool {
    live(app) || app.state::<Arc<commands::App>>().previews_open()
}

fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// The quit dialog lives in the window, so a hidden window is shown first.
pub(crate) fn request_quit(app: &AppHandle) {
    if !live(app) {
        commands::exit(app);
        return;
    }
    show_main(app);
    let _ = app.emit("quit-requested", ());
}

/// The default menu, except Quit asks first while a run is live. The built-in
/// Quit item exits without giving the app a chance to ask.
fn menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::default(app)?;
    #[cfg(target_os = "macos")]
    if let Some(MenuItemKind::Submenu(app_menu)) = menu.items()?.first() {
        let items = app_menu.items()?;
        if let Some(MenuItemKind::Predefined(_)) = items.last() {
            app_menu.remove_at(items.len() - 1)?;
        }
        app_menu.append(&MenuItem::with_id(
            app,
            QUIT,
            "Quit Shipslip",
            true,
            Some("CmdOrCtrl+Q"),
        )?)?;
    }
    Ok(menu)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    fn json(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn capability_grants_only_app_commands_window_clipboard_write_and_notify() {
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
                "allow-ack-run",
                "allow-approve-trust",
                "allow-cancel-deploy",
                "allow-cancel-pending",
                "allow-compare",
                "allow-confirm-deploy",
                "allow-current-runs",
                "allow-deploy-entry",
                "allow-detach-and-quit",
                "allow-live-runs",
                "allow-load-map",
                "allow-load-receipt",
                "allow-open-config",
                "allow-prepare-deploy",
                "allow-quit-now",
                "allow-quit-status",
                "allow-receipt-seen",
                "allow-review-attach",
                "allow-review-trust",
                "allow-review-up",
                "allow-stop-run",
                "allow-subscribe-run",
                "allow-unsubscribe-run",
                "clipboard-manager:allow-write-text",
                "core:event:default",
                "core:window:allow-start-dragging",
                "core:window:default",
                "notification:allow-is-permission-granted",
                "notification:allow-notify",
                "notification:allow-request-permission",
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
    fn window_follows_system_appearance_with_minimum_size() {
        let conf = json(include_str!("../tauri.conf.json"));
        let window = &conf["app"]["windows"][0];
        assert_eq!(window["label"], "main");
        assert!(
            window["theme"].is_null(),
            "the window follows macOS light/dark"
        );
        assert_eq!(window["titleBarStyle"], "Overlay");
        assert_eq!(window["hiddenTitle"], true);
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
