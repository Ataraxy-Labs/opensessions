//! Herdr-style manifest-driven terminal screen detection for agent panes.
//!
//! The tmux provider identifies which agent owns a pane from the foreground
//! command/title, then this module derives the live status from the visible
//! terminal tail. Rules are copied from Herdr's bundled detection manifests and
//! evaluated with the same core model: regions, nested matcher gates, rule
//! priorities, and a known-agent idle fallback.

use std::sync::OnceLock;

use regex::Regex;
use serde::Deserialize;

use crate::protocol::AgentStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentScreenDetection {
    pub status: AgentStatus,
    pub skip_state_update: bool,
    pub visible_idle: bool,
    pub visible_blocker: bool,
    pub visible_working: bool,
    pub matched_rule: Option<String>,
}

impl AgentScreenDetection {
    fn idle_fallback() -> Self {
        Self {
            status: AgentStatus::Idle,
            skip_state_update: false,
            visible_idle: false,
            visible_blocker: false,
            visible_working: false,
            matched_rule: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Manifest {
    id: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    rules: Vec<Rule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    id: String,
    state: Option<RuleState>,
    #[serde(default)]
    priority: i32,
    #[serde(default = "default_region")]
    region: String,
    #[serde(default)]
    visible_idle: bool,
    #[serde(default)]
    visible_blocker: bool,
    #[serde(default)]
    visible_working: bool,
    #[serde(default)]
    skip_state_update: bool,
    #[serde(flatten)]
    gate: Gate,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Gate {
    #[serde(default)]
    all: Vec<Gate>,
    #[serde(default)]
    any: Vec<Gate>,
    #[serde(default, rename = "not")]
    not_gate: Vec<Gate>,
    #[serde(default)]
    contains: Vec<String>,
    #[serde(default)]
    regex: Vec<String>,
    #[serde(default)]
    line_regex: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RuleState {
    Idle,
    Working,
    Blocked,
    Unknown,
}

#[derive(Debug)]
struct LoadedManifest {
    agent: &'static str,
    manifest: Manifest,
    rules: Vec<CompiledRule>,
}

#[derive(Debug)]
struct CompiledRule {
    rule_index: usize,
    gate: CompiledGate,
}

#[derive(Debug)]
struct CompiledGate {
    all: Vec<CompiledGate>,
    any: Vec<CompiledGate>,
    not_gate: Vec<CompiledGate>,
    contains: Vec<String>,
    regex: Vec<Regex>,
    line_regex: Vec<Regex>,
}

pub fn detect_agent_screen_status(agent: &str, screen: &str) -> AgentStatus {
    detect_agent_screen(agent, screen).status
}

pub fn detect_agent_screen_status_with_osc(
    agent: &str,
    screen: &str,
    osc_title: &str,
    osc_progress: &str,
) -> AgentStatus {
    detect_agent_screen_with_osc(agent, screen, osc_title, osc_progress).status
}

pub fn detect_agent_screen(agent: &str, screen: &str) -> AgentScreenDetection {
    detect_agent_screen_with_osc(agent, screen, "", "")
}

pub fn detect_agent_screen_with_osc(
    agent: &str,
    screen: &str,
    osc_title: &str,
    osc_progress: &str,
) -> AgentScreenDetection {
    let Some(loaded) = manifest_for_agent(agent) else {
        return AgentScreenDetection {
            status: AgentStatus::Idle,
            skip_state_update: false,
            visible_idle: false,
            visible_blocker: false,
            visible_working: false,
            matched_rule: None,
        };
    };

    let mut matched: Option<(&Rule, usize)> = None;
    let input = DetectionInput {
        screen,
        osc_title,
        osc_progress,
    };

    for compiled in &loaded.rules {
        let rule = &loaded.manifest.rules[compiled.rule_index];
        let region_text = region(input, &rule.region);
        if !compiled_gate_matches(&compiled.gate, region_text) {
            continue;
        }
        match matched {
            Some((previous, _)) if previous.priority >= rule.priority => {}
            _ => matched = Some((rule, compiled.rule_index)),
        }
    }

    let Some((rule, _)) = matched else {
        return AgentScreenDetection::idle_fallback();
    };
    let Some(state) = rule.state else {
        return AgentScreenDetection::idle_fallback();
    };
    let status = match state {
        RuleState::Idle => AgentStatus::Idle,
        RuleState::Working => AgentStatus::Running,
        RuleState::Blocked => AgentStatus::Waiting,
        RuleState::Unknown => AgentStatus::Idle,
    };

    AgentScreenDetection {
        status,
        skip_state_update: rule.skip_state_update,
        visible_idle: rule.visible_idle && state == RuleState::Idle,
        visible_blocker: rule.visible_blocker && state == RuleState::Blocked,
        visible_working: rule.visible_working && state == RuleState::Working,
        matched_rule: Some(rule.id.clone()),
    }
}

fn default_region() -> String {
    "whole_recent".to_string()
}

#[derive(Debug, Clone, Copy)]
struct DetectionInput<'a> {
    screen: &'a str,
    osc_title: &'a str,
    osc_progress: &'a str,
}

fn manifests() -> &'static Vec<LoadedManifest> {
    static MANIFESTS: OnceLock<Vec<LoadedManifest>> = OnceLock::new();
    MANIFESTS.get_or_init(|| {
        MANIFEST_SOURCES
            .iter()
            .map(|(agent, raw)| load_manifest(agent, raw))
            .collect()
    })
}

fn manifest_for_agent(agent: &str) -> Option<&'static LoadedManifest> {
    let normalized = normalize_agent_name(agent);
    manifests().iter().find(|loaded| {
        normalize_agent_name(loaded.agent) == normalized
            || normalize_agent_name(&loaded.manifest.id) == normalized
            || loaded
                .manifest
                .aliases
                .iter()
                .any(|alias| normalize_agent_name(alias) == normalized)
            || aliases_for_agent(loaded.agent)
                .iter()
                .any(|alias| normalize_agent_name(alias) == normalized)
    })
}

fn load_manifest(agent: &'static str, raw: &'static str) -> LoadedManifest {
    let manifest = toml::from_str::<Manifest>(raw)
        .unwrap_or_else(|err| panic!("bundled {agent} detection manifest is invalid: {err}"));
    let rules = manifest
        .rules
        .iter()
        .enumerate()
        .map(|(rule_index, rule)| CompiledRule {
            rule_index,
            gate: compile_gate(&rule.gate).unwrap_or_else(|err| {
                panic!("bundled {agent} rule {} could not compile: {err}", rule.id)
            }),
        })
        .collect();
    LoadedManifest {
        agent,
        manifest,
        rules,
    }
}

fn compile_gate(gate: &Gate) -> Result<CompiledGate, String> {
    Ok(CompiledGate {
        all: gate
            .all
            .iter()
            .map(compile_gate)
            .collect::<Result<_, _>>()?,
        any: gate
            .any
            .iter()
            .map(compile_gate)
            .collect::<Result<_, _>>()?,
        not_gate: gate
            .not_gate
            .iter()
            .map(compile_gate)
            .collect::<Result<_, _>>()?,
        contains: gate
            .contains
            .iter()
            .map(|needle| needle.to_lowercase())
            .collect(),
        regex: gate
            .regex
            .iter()
            .map(|pattern| Regex::new(pattern).map_err(|err| err.to_string()))
            .collect::<Result<_, _>>()?,
        line_regex: gate
            .line_regex
            .iter()
            .map(|pattern| Regex::new(pattern).map_err(|err| err.to_string()))
            .collect::<Result<_, _>>()?,
    })
}

fn compiled_gate_matches(gate: &CompiledGate, text: &str) -> bool {
    let lower_text = text.to_lowercase();

    if !gate
        .contains
        .iter()
        .all(|needle| lower_text.contains(needle))
    {
        return false;
    }
    if !gate.regex.iter().all(|regex| regex.is_match(text)) {
        return false;
    }
    if !gate
        .line_regex
        .iter()
        .all(|regex| text.lines().any(|line| regex.is_match(line)))
    {
        return false;
    }
    if !gate
        .all
        .iter()
        .all(|nested| compiled_gate_matches(nested, text))
    {
        return false;
    }
    if !gate.any.is_empty()
        && !gate
            .any
            .iter()
            .any(|nested| compiled_gate_matches(nested, text))
    {
        return false;
    }
    if gate
        .not_gate
        .iter()
        .any(|nested| compiled_gate_matches(nested, text))
    {
        return false;
    }
    true
}

fn region<'a>(input: DetectionInput<'a>, spec: &str) -> &'a str {
    let trimmed = spec.trim();
    match trimmed {
        "osc_title" => return input.osc_title,
        "osc_progress" => return input.osc_progress,
        _ => {}
    }

    let content = input.screen;
    match trimmed {
        "whole_recent" => content,
        "after_last_prompt_marker" => after_last_prompt_marker(content),
        "before_current_prompt_marker" => before_current_prompt_marker(content),
        "whole_recent_without_current_prompt_marker" => {
            whole_recent_without_current_prompt_marker(content)
        }
        "current_prompt_block_marker" => current_prompt_block_marker(content).unwrap_or(""),
        "after_current_prompt_block_marker" => {
            after_current_prompt_block_marker(content).unwrap_or("")
        }
        "prompt_box_body" => prompt_box_body(content).unwrap_or(""),
        "above_prompt_box" => above_prompt_box(content),
        "last_non_empty_above_prompt_box" => last_non_empty_line(above_prompt_box(content)),
        "after_last_horizontal_rule" => after_last_horizontal_rule(content),
        _ => {
            if let Some(count) = region_count(trimmed, "bottom_lines") {
                return bottom_lines(content, count);
            }
            if let Some(count) = region_count(trimmed, "bottom_non_empty_lines") {
                return bottom_non_empty_lines(content, count);
            }
            ""
        }
    }
}

fn region_count(spec: &str, name: &str) -> Option<usize> {
    spec.strip_prefix(name)
        .and_then(|rest| rest.strip_prefix('('))
        .and_then(|rest| rest.strip_suffix(')'))
        .and_then(|count| count.parse::<usize>().ok())
}

fn bottom_lines(content: &str, count: usize) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(count);
    slice_from_line_index(content, &lines, start)
}

fn bottom_non_empty_lines(content: &str, count: usize) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let Some(start_index) = lines
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, line)| !line.trim().is_empty())
        .take(count)
        .last()
        .map(|(index, _)| index)
    else {
        return "";
    };
    slice_from_line_index(content, &lines, start_index)
}

fn after_last_prompt_marker(content: &str) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let Some(index) = lines.iter().rposition(|line| codex_prompt_line(line)) else {
        return content;
    };
    slice_from_line_index(content, &lines, index + 1)
}

fn before_current_prompt_marker(content: &str) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let Some(index) = current_codex_prompt_index(&lines) else {
        return content;
    };
    let byte_offset = lines[..index]
        .iter()
        .map(|line| line.len() + 1)
        .sum::<usize>();
    &content[..byte_offset.min(content.len())]
}

fn whole_recent_without_current_prompt_marker(content: &str) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    if current_codex_prompt_index(&lines).is_some() {
        ""
    } else {
        content
    }
}

fn current_prompt_block_marker(content: &str) -> Option<&str> {
    let lines: Vec<&str> = content.lines().collect();
    let prompt_index = current_codex_prompt_index(&lines)?;
    lines[..prompt_index]
        .iter()
        .rev()
        .find(|line| codex_block_marker_line(line))
        .copied()
}

fn after_current_prompt_block_marker(content: &str) -> Option<&str> {
    let lines: Vec<&str> = content.lines().collect();
    let prompt_index = current_codex_prompt_index(&lines)?;
    let block_index = lines[..prompt_index]
        .iter()
        .rposition(|line| codex_block_marker_line(line))?;
    Some(slice_from_line_index(content, &lines, block_index))
}

fn current_codex_prompt_index(lines: &[&str]) -> Option<usize> {
    let prompt_index = lines.iter().rposition(|line| codex_prompt_line(line))?;
    if lines[prompt_index + 1..]
        .iter()
        .any(|line| codex_block_marker_line(line))
    {
        return None;
    }
    Some(prompt_index)
}

fn codex_prompt_line(line: &str) -> bool {
    line == "›" || line.starts_with("› ")
}

fn codex_block_marker_line(line: &str) -> bool {
    line.starts_with('•') || line.starts_with('■') || line.starts_with('✗') || line.starts_with('✓')
}

fn prompt_box_body(content: &str) -> Option<&str> {
    let lines: Vec<&str> = content.lines().collect();
    let top = prompt_box_top_border_index(&lines)?;
    let start = line_start_offset(content, &lines, top + 1);
    let end_index = lines[top + 1..]
        .iter()
        .position(|line| is_horizontal_rule(line))
        .map(|relative| top + 1 + relative)
        .unwrap_or(lines.len());
    let end = line_start_offset(content, &lines, end_index);
    Some(&content[start.min(content.len())..end.min(content.len())])
}

fn above_prompt_box(content: &str) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let Some(top) = prompt_box_top_border_index(&lines) else {
        return content;
    };
    let end = line_start_offset(content, &lines, top);
    &content[..end.min(content.len())]
}

fn after_last_horizontal_rule(content: &str) -> &str {
    let mut last_rule_end = 0usize;
    let mut offset = 0usize;
    for line in content.lines() {
        let next_offset = offset + line.len() + 1;
        if is_horizontal_rule(line) {
            last_rule_end = next_offset.min(content.len());
        }
        offset = next_offset;
    }
    &content[last_rule_end..]
}

fn last_non_empty_line(content: &str) -> &str {
    content
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
}

fn prompt_box_top_border_index(lines: &[&str]) -> Option<usize> {
    let mut border_count = 0;
    for index in (0..lines.len()).rev() {
        if is_horizontal_rule(lines[index]) {
            border_count += 1;
            if border_count == 2 {
                return Some(index);
            }
        }
    }
    None
}

fn is_horizontal_rule(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }

    let rule_chars = trimmed.chars().take_while(|&ch| ch == '─').count();
    if rule_chars == 0 {
        return false;
    }

    let rule_bytes = trimmed
        .char_indices()
        .nth(rule_chars)
        .map(|(index, _)| index)
        .unwrap_or(trimmed.len());
    let suffix = trimmed[rule_bytes..].trim_start();

    suffix.is_empty() || rule_chars >= 3
}

fn slice_from_line_index<'a>(content: &'a str, lines: &[&str], index: usize) -> &'a str {
    let byte_offset = line_start_offset(content, lines, index);
    &content[byte_offset.min(content.len())..]
}

fn line_start_offset(content: &str, lines: &[&str], index: usize) -> usize {
    lines[..index.min(lines.len())]
        .iter()
        .map(|line| line.len() + 1)
        .sum::<usize>()
        .min(content.len())
}

fn normalize_agent_name(agent: &str) -> String {
    agent
        .trim()
        .to_ascii_lowercase()
        .replace('_', "-")
        .replace(' ', "-")
}

fn aliases_for_agent(agent: &str) -> &'static [&'static str] {
    match agent {
        "agy" => &["antigravity", "antigravity-cli"],
        "claude" => &["claude-code", "claude.exe"],
        "copilot" => &["github-copilot", "ghcs"],
        "kilo" => &["kilo-code", "kilo code", "herdr:kilo"],
        "kimi" => &["kimi-code", "kimi code"],
        "opencode" => &["open-code", "herdr:opencode"],
        "pi" => &["herdr:pi"],
        _ => &[],
    }
}

const MANIFEST_SOURCES: &[(&str, &str)] = &[
    ("amp", include_str!("agent_manifests/amp.toml")),
    ("agy", include_str!("agent_manifests/antigravity.toml")),
    ("claude", include_str!("agent_manifests/claude.toml")),
    ("cline", include_str!("agent_manifests/cline.toml")),
    ("codex", include_str!("agent_manifests/codex.toml")),
    ("cursor", include_str!("agent_manifests/cursor.toml")),
    ("devin", include_str!("agent_manifests/devin.toml")),
    ("droid", include_str!("agent_manifests/droid.toml")),
    ("gemini", include_str!("agent_manifests/gemini.toml")),
    ("grok", include_str!("agent_manifests/grok.toml")),
    ("hermes", include_str!("agent_manifests/hermes.toml")),
    ("kilo", include_str!("agent_manifests/kilo.toml")),
    ("kimi", include_str!("agent_manifests/kimi.toml")),
    ("kiro", include_str!("agent_manifests/kiro.toml")),
    ("opencode", include_str!("agent_manifests/opencode.toml")),
    ("pi", include_str!("agent_manifests/pi.toml")),
    ("qodercli", include_str!("agent_manifests/qodercli.toml")),
    (
        "copilot",
        include_str!("agent_manifests/github-copilot.toml"),
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amp_prompt_screen_falls_back_to_idle() {
        let screen = "  Response complete.\n\n╭─100% of 272k · $1.20─────────────────────────╮\n│                                               │\n╰───────────────────────~/Projects/opensessions╯";

        assert_eq!(detect_agent_screen_status("amp", screen), AgentStatus::Idle);
    }

    #[test]
    fn amp_esc_cancel_screen_is_running_like_herdr() {
        let screen = "  ✓ Search Map the core runtime architecture\n  ⋯ Oracle ▼\n  ≈ Running tools...         Esc to cancel";

        assert_eq!(
            detect_agent_screen_status("amp", screen),
            AgentStatus::Running
        );
    }

    #[test]
    fn amp_approval_screen_is_waiting() {
        let screen = "Invoke tool shell_command?\n▸● Approve [Alt+1]\n ○ Allow All for This Session [Alt+2]\n ○ Deny with feedback [Alt+4]\nWaiting for approval...";

        assert_eq!(
            detect_agent_screen_status("amp", screen),
            AgentStatus::Waiting
        );
    }

    #[test]
    fn claude_prompt_box_screen_is_idle() {
        let screen = "Task complete.\n─────────────\n❯ \n─────────────";

        assert_eq!(
            detect_agent_screen_status("claude-code", screen),
            AgentStatus::Idle
        );
    }

    #[test]
    fn claude_osc_spinner_title_is_running() {
        assert_eq!(
            detect_agent_screen_status_with_osc("claude-code", "", "⠋ Pouncing", ""),
            AgentStatus::Running
        );
    }

    #[test]
    fn claude_permission_prompt_screen_is_waiting() {
        let screen = "Do you want to proceed?\n❯ 1. Yes\n  2. No\nEsc to cancel";

        assert_eq!(
            detect_agent_screen_status("claude-code", screen),
            AgentStatus::Waiting
        );
    }

    #[test]
    fn codex_prompt_screen_is_idle() {
        let screen = "› Summarize recent commits\n\n  ~/Projects/opensessions · main";

        assert_eq!(
            detect_agent_screen_status("codex", screen),
            AgentStatus::Idle
        );
    }

    #[test]
    fn codex_osc_title_action_required_is_waiting() {
        assert_eq!(
            detect_agent_screen_status_with_osc("codex", "", "Action Required", ""),
            AgentStatus::Waiting
        );
    }

    #[test]
    fn codex_question_screen_is_waiting() {
        let screen = "Question 1/1 (1 unanswered)\nWhat kind of code improvement do you want?\n› 1. Reduce complexity\n  2. Improve reliability\n\ntab to add notes | enter to submit answer | esc to interrupt";

        assert_eq!(
            detect_agent_screen_status("codex", screen),
            AgentStatus::Waiting
        );
    }

    #[test]
    fn opencode_permission_required_is_waiting() {
        assert_eq!(
            detect_agent_screen_status(
                "opencode",
                "△ Permission required\n↑↓ select  enter confirm"
            ),
            AgentStatus::Waiting
        );
    }

    #[test]
    fn pi_working_literal_is_running() {
        assert_eq!(
            detect_agent_screen_status("pi", "Working..."),
            AgentStatus::Running
        );
    }
}
