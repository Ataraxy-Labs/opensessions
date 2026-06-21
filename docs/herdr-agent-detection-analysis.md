# Herdr Agent Detection Analysis

Source analyzed: <https://github.com/ogulcancelik/herdr>

Local checkout used for inspection:

- Path: `/tmp/herdr-checkout.7v63Sl`
- Commit: `73b137a4ae75cb1465e8d4ddfc293ff7366d8133`

## Executive summary

Herdr's agent detection is built around Herdr-owned PTY panes, not external tmux pane discovery. Herdr owns the pane runtime, child shell, PTY I/O, terminal buffer, OSC state, and pane lifecycle. Agent detection is a per-pane loop that combines:

1. Foreground process/job detection
2. Known-agent process-name and wrapper detection
3. Terminal bottom-buffer screen matching
4. OSC title/progress matching
5. Manifest-driven state rules
6. Hook/integration authority arbitration
7. UI/API mapping from internal state to user-facing status

Internal agent state is deliberately small:

- `Idle`
- `Working`
- `Blocked`
- `Unknown`

`Done` is not an internal detector state. It is derived later from `Idle + unseen`.

## Major source files

| Area | File |
|---|---|
| Core detection types and process identity | `src/detect/mod.rs` |
| Manifest engine | `src/detect/manifest.rs` |
| Built-in manifests | `src/detect/manifests/*.toml` |
| Manifest update/cache support | `src/detect/manifest_update.rs` |
| Per-pane detection loop | `src/pane.rs` |
| Detection debounce/publish helpers | `src/pane/agent_detection.rs` |
| Terminal text / OSC extraction | `src/pane/terminal.rs` |
| Platform process abstraction | `src/platform/mod.rs` |
| Linux foreground job detection | `src/platform/linux.rs` |
| macOS foreground job detection | `src/platform/macos.rs` |
| Windows foreground job approximation | `src/platform/windows.rs` |
| Terminal state arbitration | `src/terminal/state.rs` |
| Internal events | `src/events.rs` |
| App event handling | `src/app/actions.rs` |
| API status mapping | `src/app/api_helpers.rs` |
| Agent API explain endpoint | `src/app/api/agents.rs` |
| API schema | `src/api/schema/agents.rs`, `src/api/schema/common.rs` |

## End-to-end flow

```text
Herdr-owned pane / PTY
        |
        v
Per-pane detection task
        |
        +--> foreground process/job probe
        |       |
        |       v
        |   known agent identity detection
        |
        +--> terminal bottom-buffer snapshot
        |
        +--> OSC title/progress snapshot
        |
        v
Manifest engine
        |
        v
AgentDetection {
  state,
  skip_state_update,
  visible_idle,
  visible_blocker,
  visible_working
}
        |
        v
Debounce / publish decision
        |
        v
AppEvent::StateChanged
        |
        v
TerminalState arbitration
        |
        +--> fallback screen state
        +--> hook/integration authority
        +--> metadata/session identity
        |
        v
UI/API AgentStatus
```

## Core data model

Defined in `src/detect/mod.rs`.

```rust
pub enum AgentState {
    /// Agent finished, prompt visible, nothing happening.
    Idle,
    /// Agent is actively working/processing.
    Working,
    /// Agent needs human input and is blocked on a response.
    Blocked,
    /// Plain shell or unrecognized program.
    Unknown,
}
```

The detection result is richer than just `AgentState`:

```rust
pub struct AgentDetection {
    pub state: AgentState,

    /// True when the current screen is an agent-owned viewer that shows
    /// transcript/history instead of the live prompt state.
    pub skip_state_update: bool,

    /// True when the current screen visibly shows live idle chrome.
    pub visible_idle: bool,

    /// True when the current screen visibly shows live UI chrome that needs
    /// human input.
    pub visible_blocker: bool,

    /// True when the current screen visibly shows live working chrome.
    pub visible_working: bool,
}
```

The visibility flags are used later for source arbitration and false-positive control. For example, a blocked-looking string in scrollback is weaker than a manifest rule marking `visible_blocker = true` for a live prompt region.

## Supported agent identities

The `Agent` enum currently includes:

```rust
pub enum Agent {
    Pi,
    Claude,
    Codex,
    Gemini,
    Cursor,
    Devin,
    Antigravity,
    Cline,
    Omp,
    OpenCode,
    GithubCopilot,
    Kimi,
    Kiro,
    Droid,
    Amp,
    Grok,
    Hermes,
    Kilo,
    Qodercli,
}
```

`Agent::SCREEN_MANIFEST_AGENTS` lists the agents with screen manifests. It includes most agents but not every enum variant; notably, `Omp` appears to be hook/lifecycle oriented rather than normal screen-manifest driven.

## Process-name and alias matching

`identify_agent(process_name)` normalizes a process name and maps known binary names/aliases to an agent.

Representative mappings:

```rust
"pi" => Some(Agent::Pi),
"claude" | "claude-code" => Some(Agent::Claude),
"codex" => Some(Agent::Codex),
"cursor" | "cursor-agent" => Some(Agent::Cursor),
"devin" | "devin-cli" | "devin cli" => Some(Agent::Devin),
"agy" | "antigravity" | "antigravity-cli" => Some(Agent::Antigravity),
"opencode" | "open-code" => Some(Agent::OpenCode),
"copilot" | "github-copilot" | "ghcs" => Some(Agent::GithubCopilot),
"kimi" | "kimi-code" | "kimi code" => Some(Agent::Kimi),
"kiro" | "kiro-cli" => Some(Agent::Kiro),
"amp" | "amp-local" => Some(Agent::Amp),
"grok" | "grok-build" => Some(Agent::Grok),
"hermes" | "hermes-agent" => Some(Agent::Hermes),
"kilo" | "kilo-code" | "kilo code" => Some(Agent::Kilo),
"qodercli" | "qoderclicn" | "qoder" | "qodercn" => Some(Agent::Qodercli),
```

The same alias logic is used by `parse_agent_label`, which is also used for env hints and hook labels.

## Foreground job abstraction

Platform-neutral process types live in `src/platform/mod.rs`:

```rust
pub struct ForegroundProcess {
    pub pid: u32,
    pub name: String,
    pub argv0: Option<String>,
    pub argv: Option<Vec<String>>,
    pub cmdline: Option<String>,
}

pub struct ForegroundJob {
    pub process_group_id: u32,
    pub processes: Vec<ForegroundProcess>,
}
```

Herdr asks the platform layer for the foreground job for a pane shell, then identifies an agent inside that job.

## Agent identity within a foreground job

`identify_agent_in_job(job)` in `src/detect/mod.rs` does this:

1. If the process-group leader is a known agent, use it.
2. Otherwise inspect all foreground job processes.
3. Normalize wrapped process names.
4. Score candidates and pick the best.

Simplified source:

```rust
pub fn identify_agent_in_job(job: &crate::platform::ForegroundJob) -> Option<(Agent, String)> {
    if let Some(process) = job
        .processes
        .iter()
        .find(|process| process.pid == job.process_group_id)
    {
        let candidate = normalized_process_name(process);
        if let Some(agent) = identify_agent(&candidate) {
            return Some((agent, candidate));
        }
    }

    let mut best: Option<(u8, Agent, String)> = None;

    for process in &job.processes {
        let candidate = normalized_process_name(process);
        let Some(agent) = identify_agent(&candidate) else {
            continue;
        };
        let score = process_priority(process, &candidate);

        match &best {
            Some((best_score, _, _)) if *best_score >= score => {}
            _ => best = Some((score, agent, candidate)),
        }
    }

    best.map(|(_, agent, name)| (agent, name))
}
```

## Wrapped process detection

Herdr handles agents launched through runtimes and shell wrappers.

`normalized_process_name(process)` first checks the effective name, then runtime argv, then argv0/cmdline paths.

Important wrapper logic:

```rust
match runtime.as_str() {
    "node" | "bun" => script_arg_agent_name(argv, &["-e", "--eval", "-p", "--print"], &[]),
    "python" | "python3" => script_arg_agent_name(argv, &["-c"], &["-m"]),
    "sh" | "bash" | "zsh" | "fish" => script_arg_agent_name(argv, &["-c"], &[]),
    "cmd" => windows_cmd_arg_agent_name(argv),
    "powershell" | "pwsh" => powershell_arg_agent_name(argv),
    "tmux" => None,
    _ => None,
}
```

Notable point: nested `tmux` is explicitly not inspected. If the foreground process is tmux, Herdr does not look inside it.

The tests in `src/detect/mod.rs` cover:

- Node-wrapped Codex
- Nix-wrapped Codex / Claude
- shell-wrapped Pi
- Bun-wrapped OMP
- Windows `cmd.exe /C codex.cmd`
- PowerShell `-File claude.ps1`
- pnpm/opencode wrappers
- false-positive avoidance for `python -c`, `node -e`, and `bash -c`

## Linux process detection

Linux implementation: `src/platform/linux.rs`.

Foreground job detection uses `/proc`:

```rust
pub fn foreground_job(child_pid: u32) -> Option<ForegroundJob> {
    let tpgid = foreground_process_group_id(child_pid)?;
    let mut processes = Vec::new();

    for entry in std::fs::read_dir("/proc").ok()? {
        let entry = entry.ok()?;
        let file_name = entry.file_name();
        let pid_str = file_name.to_str()?;
        if !pid_str.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }

        let pid: u32 = match pid_str.parse() {
            Ok(pid) => pid,
            Err(_) => continue,
        };

        let Some((pgrp, name)) = process_pgrp_and_comm(pid) else {
            continue;
        };
        if pgrp as u32 != tpgid {
            continue;
        }

        let argv = process_argv(pid);
        processes.push(ForegroundProcess {
            pid,
            name,
            argv0: None,
            cmdline: argv.as_ref().map(|parts| parts.join(" ")),
            argv,
        });
    }

    if processes.is_empty() {
        return None;
    }

    Some(ForegroundJob {
        process_group_id: tpgid,
        processes,
    })
}
```

Foreground process group comes from `/proc/<pid>/stat`:

```rust
pub fn foreground_process_group_id(child_pid: u32) -> Option<u32> {
    // /proc/<pid>/stat format: "pid (comm) state ppid pgrp session tty_nr tpgid ..."
    let stat = std::fs::read_to_string(format!("/proc/{child_pid}/stat")).ok()?;
    let rest = stat.get(stat.rfind(')')? + 2..)?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let tpgid: i32 = fields.get(5)?.parse().ok()?;
    (tpgid > 0).then_some(tpgid as u32)
}
```

Linux also supports an env hint:

```rust
pub fn process_agent_hint(pid: u32) -> Option<crate::detect::Agent> {
    if pid == 0 {
        return None;
    }
    let environ = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    parse_agent_env_hint(&environ)
}

fn parse_agent_env_hint(environ: &[u8]) -> Option<crate::detect::Agent> {
    for record in environ.split(|&byte| byte == 0) {
        let Some(value) = record.strip_prefix(b"HERDR_AGENT=") else {
            continue;
        };
        let value = std::str::from_utf8(value).ok()?;
        return crate::detect::parse_agent_label(value);
    }
    None
}
```

## macOS process detection

macOS implementation: `src/platform/macos.rs`.

It exposes the same logical API:

- `foreground_job(child_pid)`
- `foreground_group_leader_job(process_group_id)`
- `foreground_process_group_id(pid)`
- `process_cwd(pid)`

It uses macOS APIs including:

- `proc_pidinfo`
- `PROC_PIDTBSDINFO`
- `sysctl(KERN_PROCARGS2)`
- `PROC_PIDVNODEPATHINFO`

The main product logic does not branch on macOS directly; it calls the platform abstraction.

## Windows process detection

Windows implementation: `src/platform/windows.rs`.

Windows lacks POSIX foreground process groups, so Herdr snapshots processes and selects likely foreground descendants of the pane shell.

```rust
pub fn foreground_job(child_pid: u32) -> Option<ForegroundJob> {
    let entries = snapshot_processes();
    select_pane_foreground_job(child_pid, &entries)
}
```

Selection logic:

```rust
fn select_pane_foreground_job(
    shell_pid: u32,
    entries: &[WindowsProcessEntry],
) -> Option<ForegroundJob> {
    let shell = entries.iter().find(|entry| entry.pid == shell_pid)?;
    let shell_job = || ForegroundJob {
        process_group_id: shell_pid,
        processes: vec![foreground_process_from_entry(shell)],
    };

    let descendants = descendant_entries(shell_pid, entries);
    let mut candidates = Vec::new();
    for entry in &descendants {
        let process = foreground_process_from_entry(entry);
        let job = ForegroundJob {
            process_group_id: entry.pid,
            processes: vec![process],
        };
        if let Some((agent, _)) = crate::detect::identify_agent_in_job(&job) {
            candidates.push((*entry, agent));
        }
    }

    match candidates.len() {
        1 => candidates
            .pop()
            .map(|(entry, _)| foreground_job_from_entry(entry)),
        _ => select_single_agent_chain_candidate(&candidates, entries).map_or_else(
            || Some(shell_job()),
            |entry| Some(foreground_job_from_entry(entry)),
        ),
    }
}
```

So on Windows:

- one clear agent descendant wins
- multiple same-agent chain candidates may be accepted
- ambiguous cases fall back to shell

## Per-pane detection task

The Unix detection task is spawned in `src/pane.rs` by `spawn_basic_detection_task`.

Signature:

```rust
fn spawn_basic_detection_task(
    pane_id: PaneId,
    child_pid: Arc<AtomicU32>,
    terminal: Arc<PaneTerminal>,
    detection_content_seq: Arc<AtomicU64>,
    full_lifecycle_authority_active: Arc<AtomicBool>,
    state_events: mpsc::Sender<AppEvent>,
) -> (
    tokio::task::AbortHandle,
    Arc<Notify>,
    Arc<Mutex<Option<PendingAgentRelease>>>,
)
```

The loop maintains:

```rust
let mut agent_presence = AgentDetectionPresence::from_agent(None);
let mut state = AgentState::Unknown;
let mut last_visible_idle = false;
let mut last_visible_blocker = false;
let mut last_visible_working = false;
let mut last_visible_signal_refresh = None;
let mut last_process_check = std::time::Instant::now();
let mut last_foreground_pgid = None;
let mut has_process_probe = false;
let mut acquisition_started_at = None;
let mut last_content_change_at = None;
let mut pending_foreground_shell_clear = false;
let mut foreground_shell_exit_reported = false;
let mut release_was_active = false;
let mut last_detection_text = String::new();
let mut last_screen_scan_detection_content_seq = None;
let mut agent_startup_grace_until = None;
let mut pending_idle = PendingIdleConfirmation::default();
```

Polling cadence:

```rust
let sleep_duration = if pending_idle.active() {
    AGENT_PENDING_IDLE_RECHECK
} else {
    std::time::Duration::from_millis(300)
};
```

Detection timing constants in `src/pane/agent_detection.rs`:

```rust
pub(super) const AGENT_PENDING_IDLE_RECHECK: std::time::Duration =
    std::time::Duration::from_millis(100);
const AGENT_PENDING_IDLE_CONFIRMATIONS: u8 = 3;
pub(super) const AGENT_PENDING_IDLE_CAP: std::time::Duration =
    std::time::Duration::from_millis(700);
pub(super) const STABLE_VISIBLE_SIGNAL_REFRESH: std::time::Duration =
    std::time::Duration::from_millis(800);
pub(super) const AGENT_STARTUP_GRACE_WINDOW: std::time::Duration =
    std::time::Duration::from_secs(3);
```

## Process probe order inside the detection task

`probe_foreground_process_from_jobs` in `src/pane.rs` applies this priority:

1. Try foreground process-group leader job.
2. Try `HERDR_AGENT` hint from that job.
3. Try process-name identification from that job.
4. Load full foreground job.
5. Try env hint on process group leader.
6. Try process-group leader name.
7. Try env hints on non-leader foreground members.
8. Try any known process in foreground job.
9. Return no agent.

Simplified source:

```rust
fn probe_foreground_process_from_jobs(
    pid: u32,
    foreground_pgid: Option<u32>,
    leader_job: Option<crate::platform::ForegroundJob>,
    foreground_job: impl FnOnce() -> Option<crate::platform::ForegroundJob>,
    read_hint: impl Fn(u32) -> Option<Agent> + Copy,
) -> ProcessProbeResult {
    if let Some(job) = leader_job.as_ref() {
        if let Some(hinted) = hinted_process_probe_result(job, pid, read_hint) {
            return hinted;
        }
        if let Some((agent, process_name)) = crate::detect::identify_agent_in_job(job) {
            return process_probe_result(job, pid, agent, process_name);
        }
    }

    let foreground_job = foreground_job();
    if let Some(job) = foreground_job.as_ref() {
        if let Some(agent) = read_hint(job.process_group_id) {
            return process_probe_result(
                job,
                pid,
                agent,
                crate::detect::agent_label(agent).to_string(),
            );
        }
        if let Some((agent, process_name)) = identify_process_group_leader_in_job(job) {
            return process_probe_result(job, pid, agent, process_name);
        }
        if let Some(agent) = agent_hint_for_non_leader_foreground_job_members(job, read_hint) {
            return process_probe_result(
                job,
                pid,
                agent,
                crate::detect::agent_label(agent).to_string(),
            );
        }

        let identified = crate::detect::identify_agent_in_job(job);
        return ProcessProbeResult {
            process_group_id: Some(job.process_group_id),
            foreground_is_pane_shell: job.processes.iter().any(|process| process.pid == pid),
            agent: identified.as_ref().map(|(agent, _)| *agent),
            process_name: identified.map(|(_, process_name)| process_name),
        };
    }

    ProcessProbeResult {
        process_group_id: foreground_pgid,
        foreground_is_pane_shell: false,
        agent: None,
        process_name: None,
    }
}
```

## Agent presence hysteresis

Herdr does not clear a detected agent after one failed probe. It requires six consecutive misses.

```rust
const AGENT_MISS_CONFIRMATION_ATTEMPTS: u8 = 6;

impl AgentDetectionPresence {
    fn observe_process_probe(&mut self, identified_agent: Option<Agent>) -> bool {
        match identified_agent {
            Some(agent) => {
                self.consecutive_misses = 0;
                if Some(agent) == self.current_agent {
                    return false;
                }
                self.current_agent = Some(agent);
                true
            }
            None => {
                if self.current_agent.is_none() {
                    self.consecutive_misses = 0;
                    return false;
                }
                self.consecutive_misses = self.consecutive_misses.saturating_add(1);
                if self.consecutive_misses < AGENT_MISS_CONFIRMATION_ATTEMPTS {
                    return false;
                }
                self.current_agent = None;
                self.consecutive_misses = 0;
                true
            }
        }
    }
}
```

## Startup grace

When a new agent is detected, Herdr:

1. clears pending idle debounce
2. clears previous scan sequence
3. clears OSC state from the previous process
4. sets a 3-second startup grace
5. immediately publishes `Idle`

Relevant source:

```rust
if agent_changed {
    pending_idle.clear();
    last_screen_scan_detection_content_seq = None;

    // A new foreground agent must not inherit OSC
    // title/progress evidence from the previous process.
    terminal.clear_agent_osc_state();

    if agent.is_some() {
        agent_startup_grace_until = Some(now + AGENT_STARTUP_GRACE_WINDOW);
        state = AgentState::Idle;
        last_visible_idle = true;
        last_visible_blocker = false;
        last_visible_working = false;
        last_visible_signal_refresh = None;
        publish_state_changed_event(... AgentState::Idle ...).await;
    } else {
        agent_startup_grace_until = None;
    }
}
```

During startup grace, screen scanning is skipped unless the process exits.

## Terminal buffer extraction

Herdr reads from its own terminal model in `src/pane/terminal.rs`, not from tmux.

Public methods include:

```rust
pub fn visible_text(&self) -> String;
pub fn visible_ansi(&self) -> String;
pub fn detection_text(&self) -> String;
pub fn recent_text(&self, lines: usize) -> String;
pub fn recent_ansi(&self, lines: usize) -> String;
pub fn recent_unwrapped_text(&self, lines: usize) -> String;
pub fn recent_unwrapped_ansi(&self, lines: usize) -> String;
```

`detection_text()` uses recent bottom rows:

```rust
fn ghostty_detection_text(core: &GhosttyPaneCore) -> Result<String, crate::ghostty::Error> {
    let lines = core
        .terminal
        .rows()
        .ok()
        .map(|rows| usize::from(rows).max(1))
        .unwrap_or(DEFAULT_DETECTION_ROWS);
    ghostty_recent_text(core, lines)
}

fn ghostty_recent_text(
    core: &GhosttyPaneCore,
    lines: usize,
) -> Result<String, crate::ghostty::Error> {
    let total_rows = core.terminal.total_rows()?;
    let cols = core.terminal.cols()?;
    if total_rows == 0 || cols == 0 {
        return Ok(String::new());
    }
    let start = total_rows.saturating_sub(lines);
    let mut rows = Vec::with_capacity(total_rows.saturating_sub(start));
    for y in start..total_rows {
        rows.push(ghostty_screen_row(core, cols, y as u32)?);
    }
    trim_trailing_blank_rows(&mut rows);
    Ok(recent_text_from_rows(&rows, lines))
}
```

So detection uses the bottom/recent terminal buffer approximately corresponding to the pane height.

## OSC title/progress extraction

Herdr tracks agent-relevant OSC title/progress state:

```rust
pub fn agent_osc_title(&self) -> String {
    self.ghostty.agent_osc_title()
}

pub fn agent_osc_progress(&self) -> String {
    self.ghostty.agent_osc_progress()
}

/// Clears retained OSC title/progress evidence on foreground agent change.
pub fn clear_agent_osc_state(&self) { ... }
```

The detection loop reads them before manifest evaluation:

```rust
let osc_title = terminal.agent_osc_title();
let osc_progress = terminal.agent_osc_progress();
let Some(screen_detection) = detection_update_for_publish_with_osc(
    agent,
    &content,
    &osc_title,
    &osc_progress,
    process_exited,
) else {
    pending_idle.clear();
    continue;
};
```

## Manifest detection engine

Main file: `src/detect/manifest.rs`.

Detection input:

```rust
pub struct DetectionInput<'a> {
    pub screen: &'a str,
    pub osc_title: &'a str,
    pub osc_progress: &'a str,
}
```

Manifest shape:

```rust
pub(crate) struct AgentManifest {
    id: String,
    version: Option<ManifestVersion>,
    min_engine_version: Option<u32>,
    _updated_at: Option<String>,
    aliases: Vec<String>,
    rules: Vec<ManifestRule>,
}

struct ManifestRule {
    id: String,
    state: Option<ManifestState>,
    priority: i32,
    region: String,
    visible_idle: bool,
    visible_blocker: bool,
    visible_working: bool,
    skip_state_update: bool,
    all: Vec<ManifestGate>,
    any: Vec<ManifestGate>,
    not_gate: Vec<ManifestGate>,
    contains: Vec<String>,
    regex: Vec<String>,
    line_regex: Vec<String>,
}
```

Built-in manifest files:

- `amp.toml`
- `antigravity.toml`
- `claude.toml`
- `cline.toml`
- `codex.toml`
- `cursor.toml`
- `devin.toml`
- `droid.toml`
- `gemini.toml`
- `grok.toml`
- `hermes.toml`
- `kilo.toml`
- `kimi.toml`
- `kiro.toml`
- `opencode.toml`
- `pi.toml`
- `qodercli.toml`
- `github-copilot.toml`

## Manifest evaluation algorithm

`evaluate_loaded_manifest`:

1. Iterate all rules.
2. Extract the rule's region text.
3. Evaluate compiled matcher gates.
4. Track evaluated-rule evidence for explainability.
5. Select the highest-priority matching rule.
6. Convert matched rule to `DetectionExplain` / `AgentDetection`.
7. If no rule matches, fallback to known-agent idle.

Simplified source:

```rust
for (rule, compiled_rule) in loaded.manifest.rules.iter().zip(&loaded.compiled_rules) {
    let region_text = region(input, &rule.region);
    let matched_rule = compiled_rule_matches(compiled_rule, region_text);

    evaluated_rules.push(EvaluatedRule { ... });

    if !matched_rule {
        continue;
    }

    match matched {
        Some((previous, _)) if previous.priority >= rule.priority => {}
        _ => matched = Some((rule, rule.region.clone())),
    }
}

let Some((rule, region_name)) = matched else {
    return fallback_explain(...);
};

let state = rule.state.map(AgentState::from).unwrap_or(AgentState::Unknown);

DetectionExplain {
    agent: Some(agent_label(agent).to_string()),
    state,
    matched_rule: Some(MatchedRule { ... }),
    visible_idle: rule.visible_idle && state == AgentState::Idle,
    visible_blocker: rule.visible_blocker && state == AgentState::Blocked,
    visible_working: rule.visible_working && state == AgentState::Working,
    skip_state_update: rule.skip_state_update,
    ...
}
```

## Known-agent fallback behavior

If a known agent has no matching rule, Herdr reports `Idle`:

```rust
state: if known_agent {
    AgentState::Idle
} else {
    AgentState::Unknown
},
fallback_reason: known_agent.then(|| DEFAULT_KNOWN_AGENT_IDLE_FALLBACK.to_string()),
```

This is an explicit false-positive avoidance choice. Unknown live screen shapes for known agents are treated as idle, not blocked.

## Manifest regions

Supported regions include:

- `whole_recent`
- `after_last_prompt_marker`
- `before_current_prompt_marker`
- `whole_recent_without_current_prompt_marker`
- `current_prompt_block_marker`
- `after_current_prompt_block_marker`
- `prompt_box_body`
- `above_prompt_box`
- `last_non_empty_above_prompt_box`
- `after_last_horizontal_rule`
- `osc_title`
- `osc_progress`
- `bottom_lines(n)`
- `bottom_non_empty_lines(n)`

Region dispatch:

```rust
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
```

This is much more precise than matching the whole screen. It lets rules target live prompt zones, prompt boxes, OSC title, OSC progress, or the bottom N non-empty lines.

## Manifest validation

Complexity limits:

```rust
const MAX_RULES_PER_MANIFEST: usize = 128;
const MAX_GATE_DEPTH: usize = 8;
const MAX_TOTAL_GATES: usize = 512;
const MAX_MATCHERS_PER_GATE: usize = 32;
const MAX_TOTAL_MATCHERS: usize = 1024;
const MAX_MATCHER_CHARS: usize = 512;
```

`skip_state_update` rules are constrained:

```rust
if rule.skip_state_update {
    if rule.state != Some(ManifestState::Unknown) {
        return Err(format!(
            "rule {} uses skip_state_update without state = \"unknown\"",
            rule.id
        ));
    }
    if rule.visible_idle || rule.visible_blocker || rule.visible_working {
        return Err(format!(
            "rule {} uses skip_state_update with visible state evidence",
            rule.id
        ));
    }
}
```

So a skip-update rule must be `state = "unknown"` and cannot claim visible idle/blocker/working evidence.

## Example: Claude manifest

`src/detect/manifests/claude.toml`

OSC title spinner means working:

```toml
[[rules]]
id = "osc_title_working"
state = "working"
priority = 1100
region = "osc_title"
visible_working = true
regex = ['^[\x{2800}-\x{28FF}] ']
```

Transcript viewer skips state update:

```toml
[[rules]]
id = "transcript_viewer"
state = "unknown"
priority = 1000
region = "bottom_non_empty_lines(3)"
skip_state_update = true
contains = ["showing detailed transcript"]
any = [
  { contains = ["ctrl+o", "to toggle"] },
  { contains = ["ctrl+e", "show all"] },
  { contains = ["ctrl+e", "collapse"] },
  { contains = ["↑↓ scroll"] },
  { contains = ["? for shortcuts"] },
]
```

Live prompt box means idle:

```toml
[[rules]]
id = "live_prompt_box"
state = "idle"
priority = 950
region = "prompt_box_body"
visible_idle = true
line_regex = ['^\s*❯']
not = [
  { contains = ["enter to select"] },
  { contains = ["esc to cancel"] },
  { contains = ["tab/arrow keys"] },
  { contains = ["arrow keys to navigate"] },
  { contains = ["↑/↓ to navigate"] },
]
```

Permission prompt means blocked:

```toml
[[rules]]
id = "bash_permission_prompt"
state = "blocked"
priority = 850
region = "whole_recent"
visible_blocker = true
contains = ["do you want to proceed?"]
any = [
  { contains = ["bash command"] },
  { contains = ["bash("] },
  { contains = ["contains expansion"] },
  { contains = ["tab to amend"] },
  { contains = ["ctrl+e to explain"] },
]
```

## Example: Codex manifest

`src/detect/manifests/codex.toml`

OSC title `Action Required` means blocked:

```toml
[[rules]]
id = "osc_title_blocked"
state = "blocked"
priority = 1100
region = "osc_title"
visible_blocker = true
contains = ["Action Required"]
```

OSC title spinner means working:

```toml
[[rules]]
id = "osc_title_working"
state = "working"
priority = 1050
region = "osc_title"
visible_working = true
regex = ['^[\x{2800}-\x{28FF}] ']
```

Transcript viewer skips update:

```toml
[[rules]]
id = "transcript_viewer"
state = "unknown"
priority = 1000
region = "after_last_prompt_marker"
skip_state_update = true
contains = ["↑/↓ to scroll", "pgup/pgdn to", "home/end to jump", "q to quit"]
any = [
  { contains = ["esc to edit prev"] },
  { contains = ["esc/← to edit prev"] },
]
```

Live blocker:

```toml
[[rules]]
id = "live_strong_blocker"
state = "blocked"
priority = 900
region = "after_last_prompt_marker"
visible_blocker = true
any = [
  { contains = ["press enter to confirm or esc to cancel"] },
  { contains = ["enter to submit answer"] },
  { contains = ["enter to submit all"] },
  { contains = ["allow command?"] },
]
```

Plain non-empty OSC title means idle unless it is spinner/action required:

```toml
[[rules]]
id = "osc_title_idle"
state = "idle"
priority = 100
region = "osc_title"
visible_idle = true
regex = ['\S']
not = [
  { regex = ['^[\x{2800}-\x{28FF}]'] },
  { contains = ["Action Required"] },
]
```

## Example: Amp manifest

`src/detect/manifests/amp.toml`

Approval footer means blocked:

```toml
[[rules]]
id = "approval_footer"
state = "blocked"
priority = 300
region = "whole_recent"
visible_blocker = true
any = [
  { contains = ["waiting for approval"] },
  { contains = ["invoke tool"] },
  { contains = ["run this command?"] },
  { contains = ["allow editing file:"] },
  { contains = ["allow creating file:"] },
  { contains = ["confirm tool call"] },
  { contains = ["approve"], any = [{ contains = ["allow all for this session"] }, { contains = ["allow all for every session"] }, { contains = ["allow file for every session"] }, { contains = ["deny with feedback"] }] },
]
```

`esc to cancel` means working:

```toml
[[rules]]
id = "esc_cancel_working"
state = "working"
priority = 100
region = "whole_recent"
visible_working = true
contains = ["esc to cancel"]
```

## Screen read skipping

`src/pane/agent_detection.rs`

Herdr skips screen reads when already idle and nothing changed:

```rust
pub(super) fn should_skip_idle_screen_scan(input: IdleScreenScanSkipInput) -> bool {
    if input.state != AgentState::Idle
        || input.agent.is_none()
        || input.pending_idle_active
        || input.agent_changed
        || input.process_exited
    {
        return false;
    }

    input.current_detection_content_seq.is_some()
        && input.last_screen_scan_detection_content_seq == input.current_detection_content_seq
}
```

The content sequence increments on non-empty PTY reads:

```rust
pub(super) fn observe_detection_content_change(bytes: &[u8], detection_content_seq: &AtomicU64) {
    if !bytes.is_empty() {
        detection_content_seq.fetch_add(1, Ordering::Relaxed);
    }
}
```

## Working-to-idle debounce

`src/pane/agent_detection.rs`

Herdr delays weak working-to-idle transitions:

```rust
let is_working_to_plain_idle = previous.state == AgentState::Working
    && next.state == AgentState::Idle
    && !next.visible_idle
    && !next.visible_blocker
    && !agent_changed
    && !process_exited;
```

If this is a plain working-to-idle transition without visible idle evidence, Herdr waits until:

- three confirmations, or
- 700ms cap

This prevents flicker when a working indicator briefly disappears.

## Process exit handling

If process exit is detected, Herdr publishes an idle transition with visible idle:

```rust
if process_exited {
    return Some(crate::detect::AgentDetection {
        state: AgentState::Idle,
        skip_state_update: false,
        visible_idle: true,
        visible_blocker: false,
        visible_working: false,
    });
}
```

This makes process exit a strong completion signal.

## Publish decision

Herdr publishes detection updates when meaningful state/evidence changed:

```rust
pub(super) fn should_publish_detection_update(
    previous: DetectionPublishState,
    next: DetectionPublishState,
    agent_changed: bool,
    process_exited: bool,
    stable_visible_signal_refresh_due: bool,
) -> bool {
    next.state != previous.state
        || next.visible_idle != previous.visible_idle
        || next.visible_blocker != previous.visible_blocker
        || next.visible_working != previous.visible_working
        || agent_changed
        || process_exited
        || (stable_visible_signal_refresh_due && next.visible_blocker && previous.visible_blocker)
}
```

Stable visible blocker signals are refreshed every ~800ms while still present.

## Internal event

Detection publishes `AppEvent::StateChanged`:

```rust
pub enum AppEvent {
    StateChanged {
        pane_id: PaneId,
        agent: Option<Agent>,
        state: AgentState,
        visible_blocker: bool,
        visible_working: bool,
        process_exited: bool,
        observed_at: Instant,
    },
    HookStateReported { ... },
}
```

The app handles it in `src/app/actions.rs`:

```rust
AppEvent::StateChanged {
    pane_id,
    agent,
    state,
    visible_blocker,
    visible_working,
    process_exited,
    observed_at,
} => self
    .update_terminal_state(pane_id, |terminal| {
        Some(terminal.set_detected_state_with_screen_signals_at(
            agent,
            state,
            visible_blocker,
            false,
            visible_working,
            process_exited,
            observed_at,
        ))
    })
```

## Terminal state arbitration

Main file: `src/terminal/state.rs`.

`TerminalState` stores both fallback screen state and hook-authoritative state:

```rust
pub struct TerminalState {
    pub detected_agent: Option<Agent>,
    pub fallback_state: AgentState,
    fallback_visible_blocker: bool,
    fallback_observed_at: Option<Instant>,
    pub hook_authority: Option<HookAuthority>,
    pub agent_metadata: HashMap<String, AgentMetadata>,
    pub persisted_agent_session: Option<PersistedAgentSession>,
    pub state: AgentState,
    pub last_agent_state_change_seq: Option<u64>,
    pub revision: u64,
    ...
}
```

Detected screen state enters via:

```rust
pub fn set_detected_state_with_screen_signals_at(
    &mut self,
    agent: Option<Agent>,
    fallback_state: AgentState,
    visible_blocker: bool,
    _visible_idle: bool,
    _visible_working: bool,
    process_exited: bool,
    now: Instant,
) -> TerminalStateMutation
```

Hook authority enters via:

```rust
pub fn set_hook_authority_with_custom_status_at(
    &mut self,
    source: String,
    agent_label: String,
    state: AgentState,
    message: Option<String>,
    custom_status: Option<String>,
    session_ref: Option<AgentSessionRef>,
    seq: Option<u64>,
    now: Instant,
) -> Option<TerminalStateMutation>
```

## Full lifecycle hook authority

Defined in `src/detect/mod.rs`:

```rust
pub(crate) fn full_lifecycle_hook_authority(source: &str, agent_label: &str) -> bool {
    matches!(
        (source, agent_label),
        ("herdr:pi", "pi")
            | ("herdr:omp", "omp")
            | ("herdr:hermes", "hermes")
            | ("herdr:opencode", "opencode")
            | ("herdr:kilo", "kilo")
            | ("herdr:kimi", "kimi")
    )
}
```

When a full lifecycle hook is active, screen detection is ignored unless the process exited or the detected agent conflicts:

```rust
fn should_ignore_detected_state_under_full_lifecycle_hook(
    &self,
    detected_agent: Option<Agent>,
    process_exited: bool,
) -> bool {
    self.live_full_lifecycle_hook_authority()
        && !process_exited
        && !self.hook_authority_conflicts_with_detected_agent(detected_agent)
}
```

For non-full-lifecycle hooks, a visible blocker can override a non-blocked hook for the same detected agent:

```rust
fn visible_blocker_overrides_hook(&self) -> bool {
    if self.live_full_lifecycle_hook_authority() {
        return false;
    }
    self.fallback_visible_blocker
        && self.fallback_not_older_than_hook()
        && self.hook_authority.as_ref().is_some_and(|authority| {
            authority.state != AgentState::Blocked
                && crate::detect::parse_agent_label(&authority.agent_label)
                    == self.detected_agent
        })
}
```

Effective state is recomputed as:

```rust
let state = if self.visible_blocker_overrides_hook() {
    AgentState::Blocked
} else {
    self.hook_authority
        .as_ref()
        .map(|authority| authority.state)
        .unwrap_or(self.fallback_state)
};
```

## API and UI mapping

API status enum includes `Done`:

```rust
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}
```

Mapping is in `src/app/api_helpers.rs`:

```rust
pub(super) fn pane_agent_status(
    state: crate::detect::AgentState,
    seen: bool,
) -> crate::api::schema::AgentStatus {
    match (state, seen) {
        (crate::detect::AgentState::Idle, false) => crate::api::schema::AgentStatus::Done,
        (crate::detect::AgentState::Idle, true) => crate::api::schema::AgentStatus::Idle,
        (crate::detect::AgentState::Working, _) => crate::api::schema::AgentStatus::Working,
        (crate::detect::AgentState::Blocked, _) => crate::api::schema::AgentStatus::Blocked,
        (crate::detect::AgentState::Unknown, _) => crate::api::schema::AgentStatus::Unknown,
    }
}
```

Attention priority:

```rust
match (state, seen) {
    (Blocked, _) => 4,
    (Idle, false) => 3,
    (Working, _) => 2,
    (Idle, true) => 1,
    (Unknown, _) => 0,
}
```

So `Done` is attention-derived, not detector-derived.

## Agent explain API

`src/app/api/agents.rs` exposes explain behavior.

If a full lifecycle hook is active, screen detection explain reports it as skipped:

```rust
if terminal.full_lifecycle_hook_authority_active() {
    let explain = serde_json::json!({
        "agent": terminal.effective_agent_label().unwrap_or("unknown"),
        "state": crate::detect::manifest::agent_state_label(terminal.state),
        "screen_detection_skipped": true,
        "screen_detection_skip_reason": "full_lifecycle_hook_authority",
        "evaluated_rules": [],
        ...
    });
    return encode_success(id, ResponseResult::AgentExplain { explain });
}
```

Otherwise it reads current detection content and OSC fields and evaluates the manifest with explain mode:

```rust
let screen = pane.detection_text();
let osc_title = pane.agent_osc_title();
let osc_progress = pane.agent_osc_progress();
let explain = crate::detect::manifest::explain_with_input(
    agent,
    crate::detect::manifest::DetectionInput {
        screen: &screen,
        osc_title: &osc_title,
        osc_progress: &osc_progress,
    },
);
```

## Edge cases and safeguards

### Nested tmux is not inspected

`wrapped_agent_name_from_runtime_argv` explicitly returns `None` for `tmux`. Herdr sees tmux as the foreground process and does not inspect the agent inside it.

### Six misses before clearing agent

A known agent requires six consecutive process-probe misses before being cleared.

### Startup grace

Newly detected agents get a 3-second grace period and an immediate idle event. This avoids startup UI false positives.

### OSC evidence is cleared on agent change

When the foreground agent changes, Herdr clears retained OSC title/progress evidence to avoid classifying the new agent with old OSC state.

### Known-agent no-match fallback is idle

If a known agent has no manifest match, Herdr returns idle with fallback reason `default_known_agent_idle_fallback`.

### Transcript/history viewers skip state updates

Manifests use `skip_state_update = true` for transcript/history viewers, and validation forces such rules to be `state = "unknown"` with no visible evidence flags.

### Working-to-idle debounce

Plain working-to-idle transitions without visible idle evidence are held for up to 700ms or three confirmations.

### Process exit publishes idle

Process exit bypasses normal manifest detection and publishes idle with visible idle.

### Full lifecycle hooks suppress fallback screen state

For Pi, OMP, Hermes, OpenCode, Kilo, and Kimi hook authorities, screen fallback is ignored unless needed for process exit or conflict resolution.

### Visible blocker can override weaker hooks

For non-full-lifecycle hook authority, a fresh visible blocker for the same detected agent can override a non-blocked hook state.

## Tests

Important test areas:

### `src/detect/mod.rs`

Covers process identity and wrapper detection:

- plain aliases
- case normalization
- foreground job leader preference
- Nix wrappers
- shell wrappers
- Bun wrappers
- Windows cmd wrappers
- PowerShell wrappers
- false-positive avoidance for eval/command arguments

### `src/detect/manifest/tests.rs`

Covers manifest behavior:

- known-agent no-match fallback to idle
- rule priority
- nested gates
- `line_regex`
- local/remote manifest loading
- manifest source/version precedence
- invalid local override fallback
- cache behavior
- OSC title/progress tests for Claude and Codex
- visible blocker assertions
- transcript/update-skip behavior

### `src/pane/agent_detection.rs`

Covers detection publish decisions:

- pending idle confirmation
- visible blocker publish behavior
- content sequence tracking
- scan scheduling
- process-exit detection update behavior

### `src/terminal/state.rs`

Large test coverage for arbitration:

- hook authority overriding fallback
- full lifecycle hook behavior
- process exit clearing hook authority
- visible blocker override rules
- stale hook reports
- session refs
- detected agent conflicts
- suppression and stale session handling

## Key design conclusions

Herdr's detection quality comes from combining multiple weak and strong signals rather than trusting one source:

1. **Foreground process identity first**: screen matching is only meaningful once Herdr knows which agent is foreground.
2. **Wrapper-aware process detection**: node/bun/python/shell/cmd/powershell wrappers are handled carefully.
3. **Recent bottom-buffer text, not full scrollback**: reduces stale text false positives.
4. **OSC title/progress support**: high-quality state signal for agents that set title/progress.
5. **Manifest-driven rules**: agent-specific state detection is data-driven and updateable.
6. **Region-based matching**: rules can target prompt boxes, after-prompt content, OSC fields, bottom non-empty lines, etc.
7. **Visibility metadata**: Herdr tracks whether idle/blocker/working evidence is live and visible.
8. **Hysteresis everywhere**: miss confirmation, startup grace, working→idle debounce, stable visible refresh.
9. **Explicit hook arbitration**: full lifecycle integrations suppress screen fallback; session/metadata-only integrations do not.
10. **Done is not state**: done is derived from idle + unseen, keeping lifecycle and attention separate.
