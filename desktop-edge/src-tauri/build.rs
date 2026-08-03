// Declaring the app command surface via `AppManifest::commands` flips Tauri's
// default from "every registered command is callable by every window" to
// "commands must be explicitly granted in a capability" (see
// https://v2.tauri.app/security/capabilities/). Each name below autogenerates
// `allow-<command>` / `deny-<command>` permissions (kebab-case); the
// `capabilities/default.json` file then grants exactly the ones the `main`
// window is allowed to invoke. This is what makes the capability's
// "allowlisted IPC only" claim actually true (finding H5): the mic, secret,
// diagnostics, and audit commands are no longer implicitly reachable — they
// are named, reviewable ACL entries.
//
// IMPORTANT: this list MUST stay in sync with the `generate_handler!` macro in
// `src/lib.rs` and with `capabilities/default.json`. A command missing here (or
// in the capability) is denied at the IPC boundary.
fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "core_ping",
            "voice_smoke",
            "clipboard_smoke",
            "mic_smoke",
            "load_edge_settings",
            "save_edge_settings",
            "edge_settings_path",
            "presence_status",
            "presence_set_mode",
            "presence_set_signals",
            "presence_set_reduced_motion",
            "presence_apply_reduced_motion",
            "presence_current_modes",
            "presence_set_palette",
            "presence_set_ring_wanted",
            "presence_set_quality_tier",
            "presence_set_interactive",
            "presence_mic_status",
            "presence_mic_start",
            "presence_mic_stop",
            "assistant_think",
            "assistant_think_stream",
            "assistant_tool_ping",
            "assistant_notify_inbound",
            "assistant_backend_status",
            "assistant_test_backend",
            "assistant_save_backend",
            "assistant_audit_tail",
            "assistant_audit_verify",
            "assistant_diagnostics_bundle",
            "assistant_probe_backend",
            "presence_log_tail",
            "reveal_path_in_file_manager",
        ]),
    ))
    .expect("failed to run tauri-build");
}
