const SIDEBAR_PANE_TITLE: &str = "opensessions-sidebar";
const SIDEBAR_WIDTH_OPTION: &str = "@opensessions_width";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmuxVar {
    PaneId,
    PaneTitle,
    PaneWidth,
    WindowPanes,
    SidebarWidthOption,
}

impl TmuxVar {
    fn name(self) -> &'static str {
        match self {
            Self::PaneId => "pane_id",
            Self::PaneTitle => "pane_title",
            Self::PaneWidth => "pane_width",
            Self::WindowPanes => "window_panes",
            Self::SidebarWidthOption => SIDEBAR_WIDTH_OPTION,
        }
    }

    pub fn format(self) -> TmuxFormat {
        TmuxFormat::var(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxFormat {
    Var(TmuxVar),
    VarName(&'static str),
    Literal(String),
    Eq(Box<TmuxFormat>, Box<TmuxFormat>),
    Neq(Box<TmuxFormat>, Box<TmuxFormat>),
    Gt(Box<TmuxFormat>, Box<TmuxFormat>),
    And(Vec<TmuxFormat>),
}

impl TmuxFormat {
    pub fn var(var: TmuxVar) -> Self {
        Self::Var(var)
    }

    pub fn var_name(name: &'static str) -> Self {
        Self::VarName(name)
    }

    pub fn literal(value: impl Into<String>) -> Self {
        Self::Literal(value.into())
    }

    pub fn eq(left: TmuxFormat, right: TmuxFormat) -> Self {
        Self::Eq(Box::new(left), Box::new(right))
    }

    pub fn neq(left: TmuxFormat, right: TmuxFormat) -> Self {
        Self::Neq(Box::new(left), Box::new(right))
    }

    pub fn gt(left: TmuxFormat, right: TmuxFormat) -> Self {
        Self::Gt(Box::new(left), Box::new(right))
    }

    pub fn and(parts: impl IntoIterator<Item = TmuxFormat>) -> Self {
        let mut parts = parts.into_iter().collect::<Vec<_>>();
        if parts.len() == 1 {
            return parts.remove(0);
        }
        Self::And(parts)
    }

    pub fn render(&self) -> String {
        self.render_with_hash("#")
    }

    pub fn render_for_hook(&self) -> String {
        self.render_with_hash("##")
    }

    fn render_with_hash(&self, hash: &str) -> String {
        match self {
            Self::Var(var) => format!("{hash}{{{}}}", var.name()),
            Self::VarName(name) => format!("{hash}{{{name}}}"),
            Self::Literal(value) => value.clone(),
            Self::Eq(left, right) => format!(
                "{hash}{{==:{},{}}}",
                left.render_with_hash(hash),
                right.render_with_hash(hash)
            ),
            Self::Neq(left, right) => format!(
                "{hash}{{!=:{},{}}}",
                left.render_with_hash(hash),
                right.render_with_hash(hash)
            ),
            Self::Gt(left, right) => format!(
                "{hash}{{>:{},{}}}",
                left.render_with_hash(hash),
                right.render_with_hash(hash)
            ),
            Self::And(parts) => render_binary_operator(hash, "&&", parts),
        }
    }
}

fn render_binary_operator(hash: &str, operator: &str, parts: &[TmuxFormat]) -> String {
    match parts {
        [] => String::new(),
        [only] => only.render_with_hash(hash),
        [first, second] => format!(
            "{hash}{{{operator}:{},{}}}",
            first.render_with_hash(hash),
            second.render_with_hash(hash)
        ),
        [first, rest @ ..] => format!(
            "{hash}{{{operator}:{},{}}}",
            first.render_with_hash(hash),
            render_binary_operator(hash, operator, rest)
        ),
    }
}

pub fn hook_context_format() -> &'static str {
    "#{client_tty}|#{session_name}|#{window_id}|#{pane_id}|#{pane_active}"
}

pub fn http_hook_command(base: &str, path: &str, data: Option<&str>, background: bool) -> String {
    let body = data.map(|data| format!(" -d '{data}'")).unwrap_or_default();
    run_shell_command(
        &format!(
            "curl -s -o /dev/null -m 0.2 --connect-timeout 0.1 -X POST {base}{path}{body} >/dev/null 2>&1 || true"
        ),
        background,
    )
}

fn run_shell_command(script: &str, background: bool) -> String {
    let background = if background { " -b" } else { "" };
    format!("run-shell{background} \"{}\"", tmux_double_quote(script))
}

fn tmux_double_quote(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
}

pub fn resized_pane_width_repair_command() -> String {
    run_shell_command(
        &format!(
            "[ '{}' != 1 ] || tmux -S #{{socket_path}} resize-pane -t '{}' -x '{}'",
            sidebar_width_repair_filter().render(),
            TmuxVar::PaneId.format().render(),
            TmuxVar::SidebarWidthOption.format().render(),
        ),
        true,
    )
}

pub fn close_orphan_sidebar_pipeline() -> String {
    format!(
        "tmux -S #{{socket_path}} list-panes -a -f '{}' -F '{}' | while IFS=$(printf '\\t') read -r session pane windows; do if [ \"$windows\" -le 1 ]; then fallback=$(tmux -S #{{socket_path}} list-sessions -F '{}' | awk -v s=\"$session\" '$0==s {{ if (prev != \"\") {{ print prev; exit }}; seen=1; next }} seen {{ print; exit }} {{ prev=$0 }}'); tmux -S #{{socket_path}} list-clients -t \"=$session:\" -F '{}' | while IFS= read -r client; do [ -n \"$client\" ] && [ -n \"$fallback\" ] && tmux -S #{{socket_path}} switch-client -c \"$client\" -t \"=$fallback:\" >/dev/null 2>&1 || true; done; fi; tmux -S #{{socket_path}} kill-pane -t \"$pane\" >/dev/null 2>&1 || true; done",
        orphan_sidebar_filter().render_for_hook(),
        orphan_sidebar_row_format(),
        TmuxFormat::var_name("session_name").render_for_hook(),
        TmuxFormat::var_name("client_tty").render_for_hook(),
    )
}

pub fn pane_exited_hook_command(base: &str) -> String {
    format!(
        "{} ; {}",
        run_shell_command(&close_orphan_sidebar_pipeline(), false),
        http_hook_command(base, "/pane-exited", None, false),
    )
}

pub fn pane_died_hook_command(base: &str) -> String {
    format!(
        "{} ; {}",
        run_shell_command(&close_dead_content_pane_pipeline(), false),
        http_hook_command(base, "/pane-exited", None, false),
    )
}

pub fn close_dead_content_pane_pipeline() -> String {
    let pane_title = TmuxVar::PaneTitle.format().render_for_hook();
    let pane_dead = TmuxFormat::var_name("pane_dead").render_for_hook();
    let window_id = TmuxFormat::var_name("window_id").render_for_hook();
    let session_name = TmuxFormat::var_name("session_name").render_for_hook();
    let session_windows = TmuxFormat::var_name("session_windows").render_for_hook();
    let client_tty = TmuxFormat::var_name("client_tty").render_for_hook();
    let pane_id = TmuxVar::PaneId.format().render_for_hook();
    let dead_pane_filter =
        TmuxFormat::eq(TmuxFormat::var_name("pane_dead"), TmuxFormat::literal("1"))
            .render_for_hook();

    format!(
        "tmux -S #{{socket_path}} list-panes -a -F '{session_name}\t{window_id}\t{pane_title}\t{pane_dead}\t{session_windows}' | awk -F '\t' '{{ key=$1 \"\\t\" $2; session[key]=$1; window[key]=$2; windows[key]=$5; if ($3==\"opensessions-sidebar\") {{ sidebars[key]++ }} else if ($4!=\"1\") {{ live[key]++ }} }} END {{ for (key in session) if (sidebars[key] > 0 && live[key]+0 == 0) print session[key] \"\\t\" window[key] \"\\t\" windows[key] }}' | while IFS=$(printf '\\t') read -r session window windows; do if [ \"$windows\" -le 1 ]; then fallback=$(tmux -S #{{socket_path}} list-sessions -F '{session_name}' | awk -v s=\"$session\" '$0==s {{ if (prev != \"\") {{ print prev; exit }}; seen=1; next }} seen {{ print; exit }} {{ prev=$0 }}'); tmux -S #{{socket_path}} list-clients -t \"=$session:\" -F '{client_tty}' | while IFS= read -r client; do [ -n \"$client\" ] && [ -n \"$fallback\" ] && tmux -S #{{socket_path}} switch-client -c \"$client\" -t \"=$fallback:\" >/dev/null 2>&1 || true; done; fi; tmux -S #{{socket_path}} kill-window -t \"$window\" >/dev/null 2>&1 || true; done; tmux -S #{{socket_path}} list-panes -a -f '{dead_pane_filter}' -F '{pane_id}' | while IFS= read -r pane; do [ -n \"$pane\" ] && tmux -S #{{socket_path}} kill-pane -t \"$pane\" >/dev/null 2>&1 || true; done"
    )
}

fn orphan_sidebar_row_format() -> String {
    [
        TmuxFormat::var_name("session_name"),
        TmuxVar::PaneId.format(),
        TmuxFormat::var_name("session_windows"),
    ]
    .into_iter()
    .map(|format| format.render_for_hook())
    .collect::<Vec<_>>()
    .join("\t")
}

fn sidebar_width_repair_filter() -> TmuxFormat {
    TmuxFormat::and([
        TmuxFormat::gt(TmuxVar::WindowPanes.format(), TmuxFormat::literal("1")),
        TmuxFormat::and([
            sidebar_pane_filter(),
            TmuxFormat::neq(
                TmuxVar::PaneWidth.format(),
                TmuxVar::SidebarWidthOption.format(),
            ),
        ]),
    ])
}

fn orphan_sidebar_filter() -> TmuxFormat {
    TmuxFormat::and([
        TmuxFormat::eq(TmuxVar::WindowPanes.format(), TmuxFormat::literal("1")),
        sidebar_pane_filter(),
    ])
}

fn sidebar_pane_filter() -> TmuxFormat {
    TmuxFormat::eq(
        TmuxVar::PaneTitle.format(),
        TmuxFormat::literal(SIDEBAR_PANE_TITLE),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_tmux_formats_once_or_escaped_for_hooks() {
        let filter = TmuxFormat::and([
            TmuxFormat::gt(TmuxVar::WindowPanes.format(), TmuxFormat::literal("1")),
            TmuxFormat::eq(
                TmuxVar::PaneTitle.format(),
                TmuxFormat::literal("opensessions-sidebar"),
            ),
        ]);

        assert_eq!(
            filter.render(),
            "#{&&:#{>:#{window_panes},1},#{==:#{pane_title},opensessions-sidebar}}"
        );
        assert_eq!(
            filter.render_for_hook(),
            "##{&&:##{>:##{window_panes},1},##{==:##{pane_title},opensessions-sidebar}}"
        );
    }

    #[test]
    fn renders_resized_pane_repair_without_a_global_scan() {
        assert_eq!(
            resized_pane_width_repair_command(),
            "run-shell -b \"[ '#{&&:#{>:#{window_panes},1},#{&&:#{==:#{pane_title},opensessions-sidebar},#{!=:#{pane_width},#{@opensessions_width}}}}' != 1 ] || tmux -S #{socket_path} resize-pane -t '#{pane_id}' -x '#{@opensessions_width}'\""
        );
    }

    #[test]
    fn renders_pane_exited_hook_with_orphan_close_before_server_cleanup() {
        let hook = pane_exited_hook_command("http://127.0.0.1:1234");

        assert!(hook.starts_with(
            "run-shell \"tmux -S #{socket_path} list-panes -a -f '##{&&:##{==:##{window_panes},1},##{==:##{pane_title},opensessions-sidebar}}'"
        ));
        assert!(
            hook.contains("switch-client -c \\\"\\$client\\\" -t \\\"=\\$fallback:\\\"")
                || hook.contains("switch-client -c \"\\$client\" -t \"=\\$fallback:\"")
        );
        assert!(
            hook.contains("kill-pane -t \\\"\\$pane\\\"")
                || hook.contains("kill-pane -t \"\\$pane\"")
        );
        assert!(hook.contains("-X POST http://127.0.0.1:1234/pane-exited"));
        assert!(!hook.contains("list-panes -a -f '##{&&:##{>:"));
    }
}
