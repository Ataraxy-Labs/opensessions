use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

use crate::protocol::{LocalLink, LocalLinkKind};

const DEFAULT_PROXY_PORT: u16 = 1355;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortlessRoute {
    pub hostname: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortlessState {
    pub proxy_port: u16,
    pub secure: bool,
    pub routes_by_port: BTreeMap<u16, Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct RawRoute {
    hostname: Option<String>,
    port: Option<serde_json::Value>,
}

pub fn parse_portless_routes(text: &str) -> Vec<PortlessRoute> {
    let Ok(raw_routes) = serde_json::from_str::<Vec<Option<RawRoute>>>(text) else {
        return Vec::new();
    };

    raw_routes
        .into_iter()
        .flatten()
        .filter_map(|route| {
            let hostname = route.hostname?;
            let port = route.port?.as_u64()?;
            if port == 0 || port > u16::MAX as u64 {
                return None;
            }
            Some(PortlessRoute {
                hostname,
                port: port as u16,
            })
        })
        .collect()
}

pub fn load_portless_state(dirs: impl IntoIterator<Item = PathBuf>) -> Option<PortlessState> {
    for dir in dirs {
        let routes_path = dir.join("routes.json");
        if !routes_path.exists() {
            continue;
        }

        let routes = parse_portless_routes(&fs::read_to_string(routes_path).ok()?);
        let proxy_port = fs::read_to_string(dir.join("proxy.port"))
            .ok()
            .and_then(|value| value.trim().parse::<u16>().ok())
            .filter(|port| *port > 0)
            .unwrap_or(DEFAULT_PROXY_PORT);
        let secure = dir.join("proxy.tls").exists();

        let mut routes_by_port = BTreeMap::<u16, Vec<String>>::new();
        for route in routes {
            let hostnames = routes_by_port.entry(route.port).or_default();
            if !hostnames.contains(&route.hostname) {
                hostnames.push(route.hostname);
                hostnames.sort();
            }
        }

        return Some(PortlessState {
            proxy_port,
            secure,
            routes_by_port,
        });
    }

    None
}

pub fn build_local_links(
    ports: impl IntoIterator<Item = u16>,
    portless_state: Option<&PortlessState>,
) -> Vec<LocalLink> {
    let mut links = Vec::new();
    let mut seen = std::collections::BTreeSet::new();

    for port in ports {
        let hostnames = portless_state.and_then(|state| state.routes_by_port.get(&port));
        if let Some(hostnames) = hostnames.filter(|hostnames| !hostnames.is_empty()) {
            let state = portless_state.expect("state exists when hostnames exist");
            for hostname in hostnames {
                let url = format_url(hostname, state.proxy_port, state.secure);
                if !seen.insert(url.clone()) {
                    continue;
                }
                links.push(LocalLink {
                    kind: LocalLinkKind::Portless,
                    port: port as u32,
                    label: display_label(&url),
                    url,
                });
            }
            continue;
        }

        let url = format!("http://localhost:{port}");
        if seen.insert(url.clone()) {
            links.push(LocalLink {
                kind: LocalLinkKind::Direct,
                port: port as u32,
                url,
                label: format!("localhost:{port}"),
            });
        }
    }

    links
}

fn format_url(hostname: &str, proxy_port: u16, secure: bool) -> String {
    let protocol = if secure { "https" } else { "http" };
    let default_port = if secure { 443 } else { 80 };
    if proxy_port == default_port {
        format!("{protocol}://{hostname}")
    } else {
        format!("{protocol}://{hostname}:{proxy_port}")
    }
}

fn display_label(url: &str) -> String {
    url.strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url)
        .to_string()
}
