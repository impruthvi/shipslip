fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "load_map",
            "compare",
            "load_receipt",
            "deploy_entry",
            "prepare_deploy",
            "review_trust",
            "approve_trust",
            "confirm_deploy",
            "cancel_deploy",
            "review_up",
            "review_attach",
            "current_runs",
            "subscribe_run",
            "ack_run",
            "unsubscribe_run",
            "stop_run",
            "cancel_pending",
            "receipt_seen",
            "quit_status",
            "detach_and_quit",
            "quit_now",
        ]),
    ))
    .expect("failed to build app permissions");
}
