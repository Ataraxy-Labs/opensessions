use std::collections::{BTreeMap, BTreeSet};

pub type DirSessionMap = BTreeMap<String, Vec<String>>;

pub fn build_dir_session_map(
    sessions: impl IntoIterator<Item = (String, String)>,
) -> DirSessionMap {
    let mut map = BTreeMap::new();
    for (name, dir) in sessions {
        if dir.is_empty() {
            continue;
        }
        let names: &mut Vec<String> = map.entry(dir).or_default();
        if !names.contains(&name) {
            names.push(name);
        }
    }
    map
}

pub fn resolve_session_for_project_dir(
    project_dir: &str,
    dir_session_map: &DirSessionMap,
) -> Option<String> {
    let exact_matches = dir_session_map
        .get(project_dir)
        .map(|sessions| sessions.iter().cloned().collect::<BTreeSet<_>>())
        .unwrap_or_default();
    if !exact_matches.is_empty() {
        return unique_match(exact_matches);
    }

    let mut related_matches = BTreeSet::new();
    for (dir, sessions) in dir_session_map {
        if !project_dir.starts_with(&format!("{dir}/"))
            && !dir.starts_with(&format!("{project_dir}/"))
        {
            continue;
        }
        related_matches.extend(sessions.iter().cloned());
    }
    if !related_matches.is_empty() {
        return unique_match(related_matches);
    }

    let Some(encoded) = project_dir.strip_prefix("__encoded__:") else {
        return None;
    };

    let mut encoded_matches = BTreeSet::new();
    for (dir, sessions) in dir_session_map {
        if encode_project_dir(dir) != encoded {
            continue;
        }
        encoded_matches.extend(sessions.iter().cloned());
    }
    unique_match(encoded_matches)
}

fn unique_match(matches: BTreeSet<String>) -> Option<String> {
    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

fn encode_project_dir(dir: &str) -> String {
    dir.chars()
        .map(|ch| {
            if matches!(ch, '/' | '.' | '_') {
                '-'
            } else {
                ch
            }
        })
        .collect()
}
