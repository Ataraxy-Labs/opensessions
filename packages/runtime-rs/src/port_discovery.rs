use std::collections::{HashMap, HashSet};

pub struct PortDiscoveryInput<'a> {
    pub session_names: Vec<String>,
    pub pane_pids_by_session: HashMap<String, Vec<u32>>,
    pub process_rows: Vec<(u32, u32)>,
    pub lsof_fields: &'a str,
}

pub fn discover_session_ports(input: PortDiscoveryInput<'_>) -> HashMap<String, Vec<u16>> {
    let mut ports_by_session = input
        .session_names
        .iter()
        .map(|session| (session.clone(), HashSet::<u16>::new()))
        .collect::<HashMap<_, _>>();
    let session_filter = input.session_names.into_iter().collect::<HashSet<_>>();
    if input.pane_pids_by_session.is_empty() {
        return finalize_ports(ports_by_session);
    }

    let mut children_of = HashMap::<u32, Vec<u32>>::new();
    for (pid, ppid) in input.process_rows {
        children_of.entry(ppid).or_default().push(pid);
    }

    let mut sessions_by_pid = HashMap::<u32, Vec<String>>::new();
    for (session, pane_pids) in input.pane_pids_by_session {
        if !session_filter.contains(&session) {
            continue;
        }
        let descendants = descendants_including_roots(&pane_pids, &children_of);
        for pid in descendants {
            sessions_by_pid
                .entry(pid)
                .or_default()
                .push(session.clone());
        }
    }

    let mut current_pid = None;
    for line in input.lsof_fields.lines() {
        if let Some(pid) = line.strip_prefix('p').and_then(parse_u32) {
            current_pid = Some(pid);
            continue;
        }

        let Some(name_field) = line.strip_prefix('n') else {
            continue;
        };
        let (Some(pid), Some(port)) = (current_pid, parse_lsof_port(name_field)) else {
            continue;
        };
        let Some(sessions) = sessions_by_pid.get(&pid) else {
            continue;
        };
        for session in sessions {
            if let Some(ports) = ports_by_session.get_mut(session) {
                ports.insert(port);
            }
        }
    }

    finalize_ports(ports_by_session)
}

fn descendants_including_roots(
    roots: &[u32],
    children_of: &HashMap<u32, Vec<u32>>,
) -> HashSet<u32> {
    let mut all = roots.iter().copied().collect::<HashSet<_>>();
    let mut queue = roots.to_vec();
    while let Some(pid) = queue.pop() {
        let Some(children) = children_of.get(&pid) else {
            continue;
        };
        for child in children {
            if all.insert(*child) {
                queue.push(*child);
            }
        }
    }
    all
}

fn parse_lsof_port(name_field: &str) -> Option<u16> {
    name_field
        .rsplit_once(':')
        .and_then(|(_, port)| parse_u16(port))
}

fn parse_u32(value: &str) -> Option<u32> {
    value.parse::<u32>().ok()
}

fn parse_u16(value: &str) -> Option<u16> {
    value.parse::<u16>().ok().filter(|port| *port > 0)
}

fn finalize_ports(ports_by_session: HashMap<String, HashSet<u16>>) -> HashMap<String, Vec<u16>> {
    ports_by_session
        .into_iter()
        .map(|(session, ports)| {
            let mut ports = ports.into_iter().collect::<Vec<_>>();
            ports.sort_unstable();
            (session, ports)
        })
        .collect()
}
