fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "load_map",
            "compare",
            "load_receipt",
            "prepare_deploy",
            "review_trust",
            "approve_trust",
            "confirm_deploy",
            "cancel_deploy",
            "review_up",
            "review_attach",
            "current_runs",
            "cancel_pending",
        ]),
    ))
    .expect("failed to build app permissions");
}
