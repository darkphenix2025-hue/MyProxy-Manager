/// Known stable configuration (for Docker/Headless fallback)
/// Antigravity 4.1.31 uses Electron 39.2.3 which corresponds to Chrome 132.0.6834.160
pub const KNOWN_STABLE_VERSION: &str = "4.1.31";
const KNOWN_STABLE_ELECTRON: &str = "39.2.3";
const KNOWN_STABLE_CHROME: &str = "132.0.6834.160";

/// Native OAuth Authorization User-Agent
pub static NATIVE_OAUTH_USER_AGENT: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| format!("vscode/1.X.X (Antigravity/{})", env!("CARGO_PKG_VERSION")));

/// Global Session ID (generated once per app launch)
pub static SESSION_ID: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| uuid::Uuid::new_v4().to_string());

/// Current version (from Cargo.toml)
pub static CURRENT_VERSION: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| env!("CARGO_PKG_VERSION").to_string());

/// Returns the project version
pub fn get_current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Returns a full User-Agent string for the current version
/// "Antigravity/4.1.31 (Macintosh; Intel Mac OS X 10_15_7) Chrome/132.0.6834.160 Electron/39.2.3"
pub fn get_default_user_agent() -> String {
    format!(
        "Antigravity/{} (Macintosh; Intel Mac OS X 10_15_7) Chrome/{} Electron/{}",
        env!("CARGO_PKG_VERSION"),
        KNOWN_STABLE_CHROME,
        KNOWN_STABLE_ELECTRON
    )
}

/// Global User-Agent string, resolved from Cargo package version
pub static USER_AGENT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let platform_info = match std::env::consts::OS {
        "macos" => "Macintosh; Intel Mac OS X 10_15_7",
        "windows" => "Windows NT 10.0; Win64; x64",
        "linux" => "X11; Linux x86_64",
        _ => "X11; Linux x86_64",
    };

    format!(
        "Antigravity/{} ({}) Chrome/{} Electron/{}",
        env!("CARGO_PKG_VERSION"),
        platform_info,
        KNOWN_STABLE_CHROME,
        KNOWN_STABLE_ELECTRON
    )
});
