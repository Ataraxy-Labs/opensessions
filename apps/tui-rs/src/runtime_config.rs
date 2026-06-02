pub const DEFAULT_SERVER_PORT: u16 = 7_391;
// SERVER_PORT_BASE is duplicated here for the TUI sidebar binary (which does
// not depend on packages/runtime-rs). The canonical definition lives in
// packages/runtime-rs/src/shared.rs and must be kept in sync — both values
// describe the same port range the server binds.
pub const SERVER_PORT_BASE: u32 = 22_000;

pub fn hash_server_key(input: &str) -> u16 {
    let mut hash = 0_u32;
    for (i, byte) in input.bytes().enumerate() {
        hash = (hash + u32::from(byte) * (i as u32 + 1)) % 20_000;
    }
    hash as u16
}

pub fn resolve_server_port(server_key: Option<u16>, explicit: Option<&str>) -> u16 {
    if let Some(port) = explicit
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|port| *port > 0)
    {
        return port;
    }

    match server_key {
        Some(key) => u16::try_from(SERVER_PORT_BASE + u32::from(key)).unwrap_or_else(|_| {
            eprintln!(
                "opensessions: server_key {key} + base {SERVER_PORT_BASE} overflows port range, \
                 falling back to DEFAULT_SERVER_PORT {DEFAULT_SERVER_PORT}",
            );
            DEFAULT_SERVER_PORT
        }),
        None => DEFAULT_SERVER_PORT,
    }
}
