use std::path::{Path, PathBuf};

pub const DEFAULT_SERVER_PORT: u16 = 7_391;
pub const DEFAULT_SERVER_HOST: &str = "127.0.0.1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSettings {
    pub server_key: Option<String>,
    pub host: String,
    pub port: u16,
    pub pid_file: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxSocketPath(PathBuf);

impl TmuxSocketPath {
    pub fn from_tmux_env(tmux: &str) -> Option<Self> {
        let socket = tmux.trim().split(',').next()?.trim();
        (!socket.is_empty()).then(|| Self(PathBuf::from(socket)))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn server_key(&self) -> ServerKey {
        ServerKey(hash_server_key(&self.0.to_string_lossy()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerKey(pub u16);

impl ServerKey {
    pub fn as_u16(self) -> u16 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpensessionsEndpoint {
    pub server_key: Option<ServerKey>,
    pub host: String,
    pub port: u16,
    pub pid_file: PathBuf,
}

impl OpensessionsEndpoint {
    pub fn from_env(env: impl Fn(&str) -> Option<String>, rust_port_base: bool) -> Self {
        let socket = env("TMUX").and_then(|tmux| TmuxSocketPath::from_tmux_env(&tmux));
        let explicit_key = env("OPENSESSIONS_SERVER_KEY")
            .and_then(|value| value.trim().parse::<u16>().ok())
            .map(ServerKey);
        let server_key = explicit_key.or_else(|| socket.as_ref().map(TmuxSocketPath::server_key));
        let host = resolve_server_host(env("OPENSESSIONS_HOST").as_deref());
        let port_base = if rust_port_base { 22_000 } else { 17_000 };
        let server_key_string = server_key.map(|key| key.0.to_string());
        let port = resolve_server_port_with_base(
            server_key_string.as_deref(),
            env("OPENSESSIONS_PORT").as_deref(),
            port_base,
        );
        let pid_file = PathBuf::from(resolve_pid_file(
            server_key_string.as_deref(),
            env("OPENSESSIONS_PID_FILE").as_deref(),
        ));
        Self {
            server_key,
            host,
            port,
            pid_file,
        }
    }
}

pub fn hash_server_key(input: &str) -> u16 {
    let mut hash = 0_u32;
    for (index, byte) in input.bytes().enumerate() {
        hash = (hash + u32::from(byte) * (index as u32 + 1)) % 20_000;
    }
    hash as u16
}

pub fn resolve_server_key(env: impl Fn(&str) -> Option<String>) -> Option<String> {
    if let Some(explicit) = env("OPENSESSIONS_SERVER_KEY")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return Some(explicit);
    }

    let tmux = env("TMUX")?;
    let socket_path = tmux.trim().split(',').next()?.trim();
    if socket_path.is_empty() {
        return None;
    }

    Some(hash_server_key(socket_path).to_string())
}

pub fn resolve_server_port(server_key: Option<&str>, explicit: Option<&str>) -> u16 {
    resolve_server_port_with_base(server_key, explicit, 17_000)
}

/// Compute the port like [`resolve_server_port`] but with a configurable base.
/// Mirrors the `PORT_BASE` branch in
/// `integrations/tmux-plugin/scripts/server-common.sh` so the Rust server can
/// pin 22000+server_key when `OPENSESSIONS_RUST=1` and coexist with the TS
/// legacy server (17000+server_key) on the same tmux socket.
pub fn resolve_server_port_with_base(
    server_key: Option<&str>,
    explicit: Option<&str>,
    base: u32,
) -> u16 {
    if let Some(port) = explicit
        .and_then(|value| value.trim().parse::<u16>().ok())
        .filter(|port| *port > 0)
    {
        return port;
    }

    let Some(server_key) = server_key else {
        return DEFAULT_SERVER_PORT;
    };

    match server_key.trim().parse::<u32>() {
        Ok(key) => (base + key) as u16,
        Err(_) => DEFAULT_SERVER_PORT,
    }
}

pub fn resolve_server_host(explicit: Option<&str>) -> String {
    explicit
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_SERVER_HOST)
        .to_string()
}

pub fn resolve_pid_file(server_key: Option<&str>, explicit: Option<&str>) -> String {
    if let Some(path) = explicit.map(str::trim).filter(|value| !value.is_empty()) {
        return path.to_string();
    }

    match server_key {
        Some(key) => format!("/tmp/opensessions.{key}.pid"),
        None => "/tmp/opensessions.pid".to_string(),
    }
}

pub fn resolve_server_settings(env: impl Fn(&str) -> Option<String>) -> ServerSettings {
    let rust_port_base = env("OPENSESSIONS_RUST")
        .map(|value| value.trim() == "1")
        .unwrap_or(false);
    let endpoint = OpensessionsEndpoint::from_env(env, rust_port_base);

    ServerSettings {
        server_key: endpoint.server_key.map(|key| key.0.to_string()),
        host: endpoint.host,
        port: endpoint.port,
        pid_file: endpoint.pid_file.to_string_lossy().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::hash_server_key;

    #[test]
    fn server_key_hashes_utf8_bytes() {
        assert_eq!(hash_server_key("/private/tmp/tmux-501/default"), 19_916);
        assert_eq!(hash_server_key("/tmp/é/socket"), 11_473);
        assert_eq!(hash_server_key("/tmp/😀/socket"), 15_433);
    }
}
