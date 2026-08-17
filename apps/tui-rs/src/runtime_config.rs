pub const DEFAULT_SERVER_PORT: u16 = 7_391;
const RUST_SERVER_PORT_BASE: u16 = 22_000;

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
        Some(key) => RUST_SERVER_PORT_BASE + key,
        None => DEFAULT_SERVER_PORT,
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
