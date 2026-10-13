//! Per-user folders: XDG on Linux, %APPDATA% / %LOCALAPPDATA% / %USERPROFILE% on Windows,
//! ~/Library on macOS.

use std::path::PathBuf;

fn env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn home() -> Option<PathBuf> {
    env("HOME").or_else(|| env("USERPROFILE"))
}

/// Settings folder of the app (`…/stylized3dpaint`).
pub fn config_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        env("APPDATA")
    } else if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library").join("Application Support"))
    } else {
        env("XDG_CONFIG_HOME").or_else(|| home().map(|h| h.join(".config")))
    };
    base.map(|b| b.join("stylized3dpaint"))
}

/// Cache / log folder of the app (`…/stylized3dpaint`).
pub fn cache_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        env("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library").join("Caches"))
    } else {
        env("XDG_CACHE_HOME").or_else(|| home().map(|h| h.join(".cache")))
    };
    base.map(|b| b.join("stylized3dpaint"))
}
