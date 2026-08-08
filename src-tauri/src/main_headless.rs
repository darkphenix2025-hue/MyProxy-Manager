// Headless proxy entry point (no Tauri GUI required)
// Re-uses the same headless logic from lib.rs but without GTK dependency

use antigravity_tools_lib::run;
use std::env;

fn main() {
    // Inject --headless flag by prepending it to args
    let orig_args: Vec<String> = env::args().collect();
    let mut new_args = vec!["antigravity-proxy".to_string(), "--headless".to_string()];
    for arg in orig_args.into_iter().skip(1) {
        new_args.push(arg);
    }
    // Since Rust doesn't allow replacing argv after startup,
    // we rely on the fact that run() collects env::args() at call time.
    // But args() returns the original process args, not our modified ones.
    // So instead, we set an env var that run() also checks.
    env::set_var("__HEADLESS_PROXY", "1");

    run();
}
