use clap::Parser;

use crate::runtime_config::{DEFAULT_SERVER_PORT, hash_server_key, resolve_server_port};

const DEFAULT_SERVER_HOST: &str = "127.0.0.1";

#[derive(Debug, Clone, Parser)]
#[command(name = "opensessions-sidebar")]
pub struct Args {
    #[arg(long, default_value = "127.0.0.1")]
    pub server_host: String,
    #[arg(long, default_value_t = DEFAULT_SERVER_PORT)]
    pub server_port: u16,
}

impl Args {
    pub fn try_parse_from<I, T>(itr: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        <Self as Parser>::try_parse_from(itr)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEndpoint {
    pub server_host: String,
    pub server_port: u16,
}

pub fn resolve_endpoint_from_env<F>(env: F) -> ResolvedEndpoint
where
    F: Fn(&str) -> Option<String>,
{
    let server_key = resolve_server_key(&env);
    let explicit_port = env("OPENSESSIONS_PORT");
    let server_port = resolve_server_port(server_key, explicit_port.as_deref());
    let server_host = env("OPENSESSIONS_HOST")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_SERVER_HOST.to_string());

    ResolvedEndpoint {
        server_host,
        server_port,
    }
}

fn resolve_server_key<F>(env: &F) -> Option<u16>
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(explicit) = env("OPENSESSIONS_SERVER_KEY")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        && let Ok(parsed) = explicit.parse::<u16>()
    {
        return Some(parsed);
    }

    let tmux = env("TMUX")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())?;
    let socket_path = tmux.split(',').next()?;
    if socket_path.is_empty() {
        return None;
    }
    Some(hash_server_key(socket_path))
}
