use std::env;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{self, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

static TERMINATE: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_termination(_: libc::c_int) {
    TERMINATE.store(true, Ordering::SeqCst);
}

#[derive(Debug)]
struct Args {
    node_id: String,
    attach_command: String,
    cols: Option<u32>,
    rows: Option<u32>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("opensessions-bridge: {err}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    install_signal_handlers();
    claim_foreground_tty();
    reset_terminal(args.cols, args.rows, false);
    drain_terminal_input();

    let state_dir = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("opensessions-ssh-bridges");
    fs::create_dir_all(&state_dir)
        .map_err(|err| format!("create state dir {}: {err}", state_dir.display()))?;
    let state_file = state_dir.join(format!("{}.pid", args.node_id));
    let return_file = state_file.with_extension("pid.return");
    fs::write(&state_file, format!("{}\n", process::id()))
        .map_err(|err| format!("write state file {}: {err}", state_file.display()))?;

    let parent_pgrp = unsafe { libc::getpgrp() };
    let mut child = unsafe { spawn_foreground_child(&args.attach_command)? };
    let child_pid = child.id() as libc::pid_t;
    foreground_process_group(child_pid);

    let status_code = loop {
        if TERMINATE.load(Ordering::SeqCst) {
            terminate_process_group(child_pid);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status.code().unwrap_or(128),
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(err) => {
                foreground_process_group(parent_pgrp);
                cleanup(&state_file, &return_file);
                return Err(format!("wait for ssh child: {err}"));
            }
        }
    };

    foreground_process_group(parent_pgrp);
    reset_terminal(args.cols, args.rows, true);
    drain_terminal_input();

    let return_command = fs::read_to_string(&return_file).unwrap_or_default();
    cleanup(&state_file, &return_file);
    if !return_command.trim().is_empty() {
        exec_shell(&return_command);
    }
    process::exit(status_code);
}

fn parse_args() -> Result<Args, String> {
    let mut node_id = None;
    let mut attach_command = None;
    let mut cols = None;
    let mut rows = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--node-id" => node_id = args.next(),
            "--attach-command" => attach_command = args.next(),
            "--cols" => cols = args.next().and_then(|value| value.parse::<u32>().ok()),
            "--rows" => rows = args.next().and_then(|value| value.parse::<u32>().ok()),
            "--help" | "-h" => {
                println!(
                    "usage: opensessions-bridge --node-id <id> --attach-command <command> [--cols n --rows n]"
                );
                process::exit(0);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Args {
        node_id: node_id.ok_or_else(|| "missing --node-id".to_string())?,
        attach_command: attach_command.ok_or_else(|| "missing --attach-command".to_string())?,
        cols,
        rows,
    })
}

fn install_signal_handlers() {
    unsafe {
        let handler = handle_termination as *const () as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGHUP, handler);
        libc::signal(libc::SIGTTOU, libc::SIG_IGN);
        libc::signal(libc::SIGTTIN, libc::SIG_IGN);
        libc::signal(libc::SIGTSTP, libc::SIG_IGN);
    }
}

fn claim_foreground_tty() {
    let pgrp = unsafe { libc::getpgrp() };
    foreground_process_group(pgrp);
}

unsafe fn spawn_foreground_child(command: &str) -> Result<process::Child, String> {
    let mut child = Command::new("sh");
    child.arg("-lc").arg(command);
    unsafe {
        child.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    child
        .spawn()
        .map_err(|err| format!("spawn ssh attach command: {err}"))
}

fn foreground_process_group(pgrp: libc::pid_t) {
    if pgrp > 0 {
        unsafe {
            libc::tcsetpgrp(libc::STDIN_FILENO, pgrp);
        }
    }
}

fn terminate_process_group(pgrp: libc::pid_t) {
    if pgrp > 0 {
        unsafe {
            libc::kill(-pgrp, libc::SIGTERM);
        }
        thread::sleep(Duration::from_millis(150));
        unsafe {
            libc::kill(-pgrp, libc::SIGKILL);
        }
    }
}

fn reset_terminal(cols: Option<u32>, rows: Option<u32>, clear_screen: bool) {
    print!(
        "\u{000f}\u{001b}(B\u{001b})B\u{001b}[0m\u{001b}[?1l\u{001b}[?7h\u{001b}[?12l\u{001b}[?25h\u{001b}[?1000l\u{001b}[?1002l\u{001b}[?1003l\u{001b}[?1006l\u{001b}[?1015l\u{001b}[?1049l"
    );
    if clear_screen {
        print!("\u{001b}[2J\u{001b}[H");
    }
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let _ = Command::new("stty").arg("sane").status();
    if let (Some(cols), Some(rows)) = (cols, rows) {
        let _ = Command::new("stty")
            .args(["cols", &cols.to_string(), "rows", &rows.to_string()])
            .status();
    }
}

fn drain_terminal_input() {
    let saved = Command::new("stty")
        .arg("-g")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty());
    let Some(saved) = saved else {
        return;
    };
    let _ = Command::new("stty")
        .args(["-icanon", "min", "0", "time", "1"])
        .status();
    loop {
        let output = Command::new("dd")
            .args(["bs=1024", "count=1"])
            .output();
        let Ok(output) = output else {
            break;
        };
        if output.stdout.is_empty() {
            break;
        }
    }
    let _ = Command::new("stty").arg(saved).status();
}

fn exec_shell(command: &str) -> ! {
    let err = Command::new("sh").arg("-lc").arg(command).exec();
    eprintln!("opensessions-bridge: exec return command failed: {err}");
    process::exit(1);
}

fn cleanup(state_file: &PathBuf, return_file: &PathBuf) {
    let _ = fs::remove_file(state_file);
    let _ = fs::remove_file(return_file);
}
