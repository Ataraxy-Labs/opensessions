use std::collections::HashMap;

use opensessions_runtime::port_discovery::{PortDiscoveryInput, discover_session_ports};

#[test]
fn discover_session_ports_attributes_descendant_listener_pids() {
    let ports = discover_session_ports(PortDiscoveryInput {
        session_names: vec!["api".to_string(), "web".to_string(), "idle".to_string()],
        pane_pids_by_session: HashMap::from([
            ("api".to_string(), vec![100]),
            ("web".to_string(), vec![200]),
        ]),
        process_rows: vec![(101, 100), (102, 101), (201, 200), (999, 1)],
        lsof_fields: "p102\nn*:4549\nn127.0.0.1:3000\np201\nn[::1]:5173\np999\nn*:9999\np102\nn*:3000\n",
    });

    assert_eq!(ports.get("api"), Some(&vec![3000, 4549]));
    assert_eq!(ports.get("web"), Some(&vec![5173]));
    assert_eq!(ports.get("idle"), Some(&Vec::new()));
    assert!(!ports.contains_key("unknown"));
}

#[test]
fn discover_session_ports_returns_empty_session_ports_without_panes() {
    let ports = discover_session_ports(PortDiscoveryInput {
        session_names: vec!["api".to_string()],
        pane_pids_by_session: HashMap::new(),
        process_rows: vec![(101, 100)],
        lsof_fields: "p101\nn*:4549\n",
    });

    assert_eq!(ports, HashMap::from([("api".to_string(), Vec::new())]));
}
