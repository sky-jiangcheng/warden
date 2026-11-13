// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Phase 6's `init`: a terminal-level preflight that reports what the
    // machine has before the GUI ever opens. Anything without a recognized
    // subcommand still launches the window — an unrelated flag must not
    // block the app.
    if std::env::args().skip(1).any(|arg| arg == "init") {
        // The checks are async (the memory-layer handshake starts a
        // process); the CLI has no runtime of its own, so it borrows the
        // one Tauri already provides.
        let code = tauri::async_runtime::block_on(repotory_agent_lib::init::run_init());
        std::process::exit(code);
    }
    repotory_agent_lib::run()
}
