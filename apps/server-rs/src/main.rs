use opensessions_runtime::shared::resolve_server_settings;
use opensessions_server::{
    ServerConfig, default_state_source_from_env, running_server_pid, start_server,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let settings = resolve_server_settings(|key| std::env::var(key).ok());
    if let Some(pid) = running_server_pid(std::path::Path::new(&settings.pid_file)) {
        eprintln!("opensessions: another server is already running (pid {pid}). Exiting.");
        return Ok(());
    }
    let mut config = ServerConfig::new(settings.host, settings.port, settings.pid_file);
    if let Some(source) = default_state_source_from_env(|key| std::env::var(key).ok()) {
        config = config.with_state_source(source);
    }
    let server = start_server(config).await?;
    server.wait_shutdown().await?;
    Ok(())
}
