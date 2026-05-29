//! macOS system-appearance helpers.
//!
//! Parity with the TypeScript `system-theme` module: detect the macOS Appearance
//! (Light/Dark), map it to a configured theme name, and (see [`watch_mac_system_appearance`])
//! push changes to the consumer. All functions are total and macOS-gated; on
//! non-macOS platforms appearance is always [`SystemAppearance::Light`].

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemAppearance {
    Dark,
    Light,
}

/// Map a detected appearance + configured theme names to the theme to apply.
/// Pure and trivially testable.
pub fn theme_for_system_mode(
    mode: SystemAppearance,
    dark_theme: &str,
    light_theme: &str,
) -> String {
    match mode {
        SystemAppearance::Dark => dark_theme.to_string(),
        SystemAppearance::Light => light_theme.to_string(),
    }
}

/// Read the current macOS Appearance. `defaults read -g AppleInterfaceStyle`
/// prints "Dark" in dark mode and exits non-zero / empty in light mode (the key
/// is absent), so both absent and unreadable map to Light. Never panics.
#[cfg(target_os = "macos")]
pub fn read_mac_system_appearance() -> SystemAppearance {
    use std::process::Command;
    match Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
    {
        Ok(output) => {
            let value = String::from_utf8_lossy(&output.stdout);
            if value.trim() == "Dark" {
                SystemAppearance::Dark
            } else {
                SystemAppearance::Light
            }
        }
        Err(_) => SystemAppearance::Light,
    }
}

/// Non-macOS platforms have no system Appearance; always Light.
#[cfg(not(target_os = "macos"))]
pub fn read_mac_system_appearance() -> SystemAppearance {
    SystemAppearance::Light
}

/// Handle for a running appearance watcher. [`stop`](AppearanceWatcher::stop)
/// signals its threads to exit; dropping the handle does not (threads hold their
/// own stop flag), so callers keep it alive for the server's lifetime.
pub struct AppearanceWatcher {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl AppearanceWatcher {
    /// Idempotent: signals the watcher threads to exit on their next tick.
    pub fn stop(&self) {
        self.stop
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Non-macOS: nothing to watch. Returns a handle whose `stop()` is a no-op.
#[cfg(not(target_os = "macos"))]
pub fn watch_mac_system_appearance<F>(
    _on_change: F,
    _safety_poll_ms: Option<u64>,
) -> AppearanceWatcher
where
    F: Fn(SystemAppearance) + Send + Sync + 'static,
{
    AppearanceWatcher {
        stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
    }
}

/// Watch the macOS Appearance and invoke `on_change` whenever it flips.
///
/// Push-based: watches `~/Library/Preferences/.GlobalPreferences.plist` (which
/// macOS rewrites on any global-preference change) and re-reads appearance on
/// each event, suppressing the callback unless the value actually changed. A
/// safety-poll thread (default 60s) covers the atomic-rename case where the file
/// watch loses the inode. Fires once synchronously with the initial mode.
#[cfg(target_os = "macos")]
pub fn watch_mac_system_appearance<F>(
    on_change: F,
    safety_poll_ms: Option<u64>,
) -> AppearanceWatcher
where
    F: Fn(SystemAppearance) + Send + Sync + 'static,
{
    use notify::{RecursiveMode, Watcher};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    let stop = Arc::new(AtomicBool::new(false));
    let last: Arc<Mutex<Option<SystemAppearance>>> = Arc::new(Mutex::new(None));
    let on_change: Arc<dyn Fn(SystemAppearance) + Send + Sync> = Arc::new(on_change);

    let check = move || {
        let mode = read_mac_system_appearance();
        let mut guard = last.lock().unwrap();
        if *guard != Some(mode) {
            *guard = Some(mode);
            drop(guard);
            on_change(mode);
        }
    };

    // Fire once so the consumer learns the starting mode without waiting.
    check();

    let plist = home_dir().join("Library/Preferences/.GlobalPreferences.plist");

    // Push: re-check on any write to the global-preferences plist.
    {
        let check = check.clone();
        let stop = stop.clone();
        std::thread::spawn(move || {
            let (tx, rx) = std::sync::mpsc::channel();
            let Ok(mut watcher) = notify::recommended_watcher(move |res| {
                let _ = tx.send(res);
            }) else {
                return;
            };
            if watcher.watch(&plist, RecursiveMode::NonRecursive).is_err() {
                return;
            }
            while !stop.load(Ordering::SeqCst) {
                match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(_) => check(),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
    }

    // Safety poll for the atomic-rename case where the file watch goes silent.
    {
        let check = check.clone();
        let stop = stop.clone();
        let poll = Duration::from_millis(safety_poll_ms.unwrap_or(60_000));
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                std::thread::sleep(poll);
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                check();
            }
        });
    }

    AppearanceWatcher { stop }
}

#[cfg(target_os = "macos")]
fn home_dir() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default()
}
