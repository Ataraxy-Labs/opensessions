//! Shared append-only debug log for live diagnosis.
//!
//! Every process (server, sidebars, providers) appends to one file, so two
//! properties matter:
//! - Each line is written with a single `write_all` on an `O_APPEND` fd so
//!   concurrent writers never interleave mid-line.
//! - The file is size-capped: once it exceeds [`MAX_LOG_BYTES`] it is
//!   truncated in place, keeping long-running sessions from eating tmpfs
//!   (observed 2 GB before the cap existed).

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

/// Truncate the shared log once it grows past this size (64 MiB holds a day+
/// of switch/width/agent traffic at current volumes).
pub const MAX_LOG_BYTES: u64 = 64 * 1024 * 1024;

/// Resolve the log path. Defaults to `/tmp/opensessions-debug.log` so live
/// issues can be diagnosed without extra env setup; `OPENSESSIONS_DEBUG_LOG`
/// overrides the path, and an empty value disables logging.
pub fn log_path() -> Option<String> {
    let path = std::env::var("OPENSESSIONS_DEBUG_LOG")
        .ok()
        .unwrap_or_else(|| "/tmp/opensessions-debug.log".to_string());
    if path.is_empty() { None } else { Some(path) }
}

/// Append one tagged, timestamped line, e.g. `[1785…] [server pid=42] msg`.
pub fn log_with_tag(tag: &str, line: impl AsRef<str>) {
    let Some(path) = log_path() else {
        return;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    if file
        .metadata()
        .is_ok_and(|metadata| metadata.len() > MAX_LOG_BYTES)
    {
        // In-place truncation: O_APPEND writers (including other processes)
        // continue at the new EOF, so nobody needs coordination.
        let _ = file.set_len(0);
    }
    let entry = format!("[{now}] [{tag}] {}\n", line.as_ref());
    let _ = file.write_all(entry.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_log_truncates_before_append() {
        let path = std::env::temp_dir().join(format!(
            "opensessions-debug-log-cap-{}",
            std::process::id()
        ));
        let big = vec![b'x'; (MAX_LOG_BYTES + 1) as usize];
        std::fs::write(&path, &big).expect("seed oversized log");

        // SAFETY: test-local env mutation; no concurrent env readers in this test binary.
        unsafe { std::env::set_var("OPENSESSIONS_DEBUG_LOG", &path) };
        log_with_tag("test", "after-cap");
        unsafe { std::env::remove_var("OPENSESSIONS_DEBUG_LOG") };

        let contents = std::fs::read_to_string(&path).expect("read log");
        let _ = std::fs::remove_file(&path);
        assert!(
            contents.ends_with("[test] after-cap\n") && contents.len() < 128,
            "log should hold only the fresh line, got {} bytes",
            contents.len(),
        );
    }
}
