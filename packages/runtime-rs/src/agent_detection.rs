use crate::agent_screen::{AgentScreenDetection, detect_agent_screen_with_osc};
use crate::protocol::AgentStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneObservation {
    pub command: String,
    pub title: String,
    pub visible_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPaneDetection {
    pub agent: Option<String>,
    pub mapping_reason: String,
    pub status: Option<AgentStatus>,
    pub thread_name: Option<String>,
    pub screen: Option<AgentScreenDetection>,
    pub visible_tail: Vec<String>,
}

pub fn detect_agent_pane(observation: &PaneObservation) -> AgentPaneDetection {
    let mapping = identify_agent_pane(observation);
    let screen = mapping.agent.as_deref().map(|agent| {
        detect_agent_screen_with_osc(agent, &observation.visible_text, &observation.title, "")
    });
    let thread_name = mapping
        .agent
        .as_deref()
        .and_then(|agent| thread_name_from_title(&observation.title, agent));

    AgentPaneDetection {
        agent: mapping.agent,
        mapping_reason: mapping.reason,
        status: screen.as_ref().map(|screen| screen.status),
        thread_name,
        screen,
        visible_tail: visible_tail_lines(&observation.visible_text, 8),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentPaneMapping {
    agent: Option<String>,
    reason: String,
}

fn identify_agent_pane(observation: &PaneObservation) -> AgentPaneMapping {
    let command = observation.command.to_ascii_lowercase();
    if command == "pi" {
        return AgentPaneMapping {
            agent: Some("pi".to_string()),
            reason: "exact command pi".to_string(),
        };
    }
    for (agent, aliases) in AGENT_ALIASES {
        if let Some(alias) = aliases.iter().copied().find(|alias| command == *alias) {
            return AgentPaneMapping {
                agent: Some((*agent).to_string()),
                reason: format!("exact command {alias}"),
            };
        }
    }

    let title = observation.title.to_ascii_lowercase();
    if title == "pi" || title.starts_with("pi ") || title.starts_with('π') {
        return AgentPaneMapping {
            agent: Some("pi".to_string()),
            reason: "anchored title pi".to_string(),
        };
    }
    for (agent, aliases) in AGENT_ALIASES {
        if let Some(alias) = aliases
            .iter()
            .copied()
            .find(|alias| title_starts_with_agent(&title, alias))
        {
            return AgentPaneMapping {
                agent: Some((*agent).to_string()),
                reason: format!("anchored title {alias}"),
            };
        }
    }

    AgentPaneMapping {
        agent: None,
        reason: format!(
            "unmapped: command {:?}, title {:?} did not match exact command or anchored title aliases",
            observation.command, observation.title
        ),
    }
}

fn title_starts_with_agent(title: &str, alias: &str) -> bool {
    title == alias
        || title
            .strip_prefix(alias)
            .is_some_and(|rest| matches!(rest.chars().next(), Some(' ') | Some('-') | Some(':')))
}

// Keep exact process aliases as the primary pane identity. Title fallback is
// intentionally anchored because titles are UI text and can contain unrelated
// prose. Screen parsing derives live status only after pane identity is known.
const AGENT_ALIASES: &[(&str, &[&str])] = &[
    ("amp", &["amp", "amp-local"]),
    ("claude-code", &["claude", "claude-code", "claude.exe"]),
    ("codex", &["codex"]),
    ("gemini", &["gemini"]),
    ("cursor", &["cursor", "cursor-agent"]),
    ("devin", &["devin", "devin-cli"]),
    ("antigravity", &["agy", "antigravity", "antigravity-cli"]),
    ("cline", &["cline"]),
    ("opencode", &["opencode", "open-code"]),
    ("github-copilot", &["copilot", "github-copilot", "ghcs"]),
    ("kilo", &["kilo", "kilo-code"]),
    ("kimi", &["kimi", "kimi-code"]),
    ("kiro", &["kiro", "kiro-cli"]),
    ("droid", &["droid"]),
    ("grok", &["grok", "grok-build"]),
    ("hermes", &["hermes", "hermes-agent"]),
    ("qodercli", &["qodercli", "qoderclicn", "qoder", "qodercn"]),
];

fn thread_name_from_title(title: &str, agent: &str) -> Option<String> {
    let title = title.trim();
    if agent == "amp"
        && let Some((thread_name, _)) = title.split_once(" - amp - ")
    {
        let thread_name = thread_name.trim();
        if !thread_name.is_empty() && !is_amp_live_status_title(thread_name) {
            return Some(thread_name.to_string());
        }
    }
    if agent == "amp"
        && let Some(rest) = title.strip_prefix("amp - ")
        && let Some((thread_name, _)) = rest.split_once(" - ")
    {
        let thread_name = thread_name.trim();
        if !thread_name.is_empty() && !is_amp_live_status_title(thread_name) {
            return Some(thread_name.to_string());
        }
    }
    None
}

fn is_amp_live_status_title(title: &str) -> bool {
    let trimmed = title.trim_start();
    let Some(first) = trimmed.chars().next() else {
        return false;
    };
    matches!(first, '\u{2800}'..='\u{28ff}')
}

pub fn visible_tail_lines(screen: &str, max_lines: usize) -> Vec<String> {
    let mut lines = screen
        .lines()
        .rev()
        .filter_map(|line| {
            let trimmed = line.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
        .take(max_lines)
        .collect::<Vec<_>>();
    lines.reverse();
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_amp_from_anchored_title_and_screen() {
        let detection = detect_agent_pane(&PaneObservation {
            command: "bash".to_string(),
            title: "amp - fake-tools - /repo".to_string(),
            visible_text: "Esc to cancel".to_string(),
        });

        assert_eq!(detection.agent.as_deref(), Some("amp"));
        assert_eq!(detection.status, Some(AgentStatus::Running));
        assert_eq!(detection.thread_name.as_deref(), Some("fake-tools"));
    }

    #[test]
    fn command_identity_beats_title_text() {
        let detection = detect_agent_pane(&PaneObservation {
            command: "codex".to_string(),
            title: "random prose mentioning amp".to_string(),
            visible_text: "Question 1/1\nenter to submit answer".to_string(),
        });

        assert_eq!(detection.agent.as_deref(), Some("codex"));
        assert_eq!(detection.status, Some(AgentStatus::Waiting));
        assert_eq!(detection.mapping_reason, "exact command codex");
    }
}
