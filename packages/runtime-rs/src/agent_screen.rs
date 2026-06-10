use crate::protocol::AgentStatus;

pub fn detect_agent_screen_status(agent: &str, screen: &str) -> AgentStatus {
    let normalized_agent = agent.trim().to_ascii_lowercase();

    match normalized_agent.as_str() {
        "amp" => detect_amp_screen_status(screen),
        "claude" | "claude-code" => detect_claude_screen_status(screen),
        "codex" => detect_codex_screen_status(&screen.to_ascii_lowercase()),
        _ => AgentStatus::Idle,
    }
}

fn detect_amp_screen_status(screen: &str) -> AgentStatus {
    let lower = screen.to_ascii_lowercase();

    let has_waiting_for_approval = lower.contains("waiting for approval");
    let has_approval_header = lower.contains("invoke tool")
        || lower.contains("run this command?")
        || lower.contains("allow editing file:")
        || lower.contains("allow creating file:")
        || lower.contains("confirm tool call");
    let has_approval_actions = lower.contains("approve")
        && (lower.contains("allow all for this session")
            || lower.contains("allow all for every session")
            || lower.contains("allow file for every session")
            || lower.contains("deny with feedback"));

    if has_approval_actions && (has_waiting_for_approval || has_approval_header) {
        return AgentStatus::Waiting;
    }

    if lower.contains("running tools") {
        return AgentStatus::ToolRunning;
    }

    AgentStatus::Idle
}

fn detect_claude_screen_status(screen: &str) -> AgentStatus {
    let lower = screen.to_ascii_lowercase();

    if lower.contains("⌕ search…") || lower.contains("ctrl+r to toggle") {
        return AgentStatus::Idle;
    }

    if has_claude_blocked_prompt(screen, &lower) {
        return AgentStatus::Waiting;
    }

    if has_claude_working_chrome(screen) {
        return AgentStatus::Running;
    }

    AgentStatus::Idle
}

fn has_claude_blocked_prompt(content: &str, lower: &str) -> bool {
    has_confirmation_prompt(lower)
        || lower.contains("do you want to proceed?")
        || lower.contains("would you like to proceed?")
        || lower.contains("waiting for permission")
        || lower.contains("do you want to allow this connection?")
        || lower.contains("tab to amend")
        || lower.contains("ctrl+e to explain")
        || lower.contains("chat about this")
        || lower.contains("review your answers")
        || lower.contains("skip interview and plan immediately")
        || (has_selection_prompt(content) && has_claude_yes_no_choice(content))
}

fn has_confirmation_prompt(lower: &str) -> bool {
    lower.contains("confirm")
        && (lower.contains("yes") || lower.contains("allow") || lower.contains("approve"))
        && (lower.contains("no") || lower.contains("deny") || lower.contains("reject"))
}

fn has_selection_prompt(content: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with('❯') || trimmed.starts_with('>') || trimmed.starts_with("1.")
    })
}

fn has_claude_yes_no_choice(content: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line
            .trim()
            .trim_start_matches('❯')
            .trim_start()
            .to_ascii_lowercase();
        trimmed == "yes"
            || trimmed == "no"
            || trimmed.starts_with("1. yes")
            || trimmed.starts_with("2. no")
            || trimmed.starts_with("yes, and ")
            || trimmed.starts_with("no, and tell claude")
    })
}

fn has_claude_working_chrome(content: &str) -> bool {
    let above = content_above_claude_prompt_box(content);
    let above_lower = above.to_ascii_lowercase();
    above_lower.contains("esc to interrupt")
        || above_lower.contains("ctrl+c to interrupt")
        || has_claude_spinner_activity(above)
}

fn has_claude_spinner_activity(content: &str) -> bool {
    const SPINNER_CHARS: &str = "·✱✲✳✴✵✶✷✸✹✺✻✼✽✾✿❀❁❂❃❇❈❉❊❋✢✣✤✥✦✧✨⊛⊕⊙◉◎◍⁂⁕※⍟☼★☆";
    content.lines().any(|line| {
        let trimmed = line.trim();
        let Some(first) = trimmed.chars().next() else {
            return false;
        };
        if !SPINNER_CHARS.contains(first) {
            return false;
        }
        let rest = &trimmed[first.len_utf8()..];
        rest.starts_with(' ')
            && rest.contains('\u{2026}')
            && rest.chars().any(|ch| ch.is_alphanumeric())
    })
}

fn content_above_claude_prompt_box(content: &str) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let Some(top_border_index) = claude_prompt_box_top_border_index(&lines) else {
        return content;
    };

    let byte_offset: usize = lines[..top_border_index]
        .iter()
        .map(|line| line.len() + 1)
        .sum();
    &content[..byte_offset.min(content.len())]
}

fn claude_prompt_box_top_border_index(lines: &[&str]) -> Option<usize> {
    let mut border_count = 0;

    for i in (0..lines.len()).rev() {
        if is_horizontal_rule(lines[i]) {
            border_count += 1;
            if border_count == 2 {
                return Some(i);
            }
        }
    }

    None
}

fn is_horizontal_rule(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && trimmed.chars().all(|ch| ch == '─')
}

fn detect_codex_screen_status(screen: &str) -> AgentStatus {
    if screen.contains("enter to submit answer")
        || screen.contains("enter to submit all")
        || screen.contains("question 1/")
        || screen.contains("allow command")
    {
        return AgentStatus::Waiting;
    }
    if screen.contains("• working") && screen.contains("esc to interrupt") {
        return AgentStatus::Running;
    }
    AgentStatus::Idle
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amp_prompt_screen_is_idle() {
        let screen = "  Response complete.\n\n╭─100% of 272k · $1.20─────────────────────────╮\n│                                               │\n╰───────────────────────~/Projects/opensessions╯";

        assert_eq!(detect_agent_screen_status("amp", screen), AgentStatus::Idle);
    }

    #[test]
    fn amp_running_tools_screen_is_tool_running() {
        let screen = "  ✓ Search Map the core runtime architecture\n  ⋯ Oracle ▼\n  ≈ Running tools...         Esc to cancel";

        assert_eq!(
            detect_agent_screen_status("amp", screen),
            AgentStatus::ToolRunning
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
    fn claude_working_screen_is_running() {
        let screen = "✻ Pouncing…\nEsc to interrupt";

        assert_eq!(
            detect_agent_screen_status("claude-code", screen),
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
    fn codex_working_screen_is_running() {
        let screen = "• Working (17s • esc to interrupt)\n\n› Implement {feature}";

        assert_eq!(
            detect_agent_screen_status("codex", screen),
            AgentStatus::Running
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
}
