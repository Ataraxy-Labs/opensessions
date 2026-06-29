import { describe, test, expect } from "bun:test";
import { TmuxClient, TmuxError, tmux } from "../src/index";

describe("TmuxClient", () => {
  const client = tmux();

  class TestTmuxClient extends TmuxClient {
    readonly calls: readonly string[][] = [];

    override run(args: readonly string[]) {
      (this.calls as string[][]).push([...args]);
      return {
        args: ["tmux", ...args],
        exitCode: 0,
        stdout: "%9\tdev\t@3\t1\t2\t1\t/dev/ttys001\t123\t/tmp\tbash\topensessions\t26\t80\t0\t25",
        stderr: "",
        ok: true,
      };
    }
  }

  test("factory returns a TmuxClient instance", () => {
    expect(client).toBeInstanceOf(TmuxClient);
  });

  test("run() returns TmuxRunResult shape", () => {
    const result = client.run(["list-sessions", "-F", "#{session_name}"]);
    expect(result).toHaveProperty("args");
    expect(result).toHaveProperty("exitCode");
    expect(result).toHaveProperty("stdout");
    expect(result).toHaveProperty("stderr");
    expect(result).toHaveProperty("ok");
    expect(typeof result.exitCode).toBe("number");
    expect(typeof result.stdout).toBe("string");
    expect(typeof result.ok).toBe("boolean");
  });

  test("run() with throwOnError throws TmuxError on failure", () => {
    expect(() => {
      client.run(["totally-invalid-command-xyz"], { throwOnError: true });
    }).toThrow(TmuxError);
  });

  test("run() without throwOnError returns ok=false on failure", () => {
    const result = client.run(["totally-invalid-command-xyz"]);
    expect(result.ok).toBe(false);
  });

  test("listSessions() returns typed SessionInfo[]", () => {
    const sessions = client.listSessions();
    expect(Array.isArray(sessions)).toBe(true);
    for (const s of sessions) {
      expect(typeof s.id).toBe("string");
      expect(typeof s.name).toBe("string");
      expect(typeof s.createdAt).toBe("number");
      expect(typeof s.attachedClients).toBe("number");
      expect(typeof s.windowCount).toBe("number");
      expect(typeof s.dir).toBe("string");
    }
  });

  test("listWindows() returns typed WindowInfo[]", () => {
    const windows = client.listWindows();
    expect(Array.isArray(windows)).toBe(true);
    for (const w of windows) {
      expect(typeof w.id).toBe("string");
      expect(typeof w.sessionName).toBe("string");
      expect(typeof w.index).toBe("number");
      expect(typeof w.name).toBe("string");
      expect(typeof w.active).toBe("boolean");
      expect(typeof w.paneCount).toBe("number");
    }
  });

  test("listPanes() returns typed PaneInfo[]", () => {
    const panes = client.listPanes();
    expect(Array.isArray(panes)).toBe(true);
    for (const p of panes) {
      expect(typeof p.id).toBe("string");
      expect(typeof p.sessionName).toBe("string");
      expect(typeof p.windowId).toBe("string");
      expect(typeof p.title).toBe("string");
      expect(typeof p.width).toBe("number");
      expect(typeof p.height).toBe("number");
      expect(typeof p.left).toBe("number");
      expect(typeof p.right).toBe("number");
      expect(typeof p.active).toBe("boolean");
    }
  });

  test("listPanes({ scope: 'session' }) scopes to session", () => {
    const sessions = client.listSessions();
    if (sessions.length === 0) return; // skip if no sessions
    const panes = client.listPanes({ scope: "session", target: sessions[0]!.name });
    expect(Array.isArray(panes)).toBe(true);
    for (const p of panes) {
      expect(p.sessionName).toBe(sessions[0]!.name);
    }
  });

  test("listClients() returns typed ClientInfo[]", () => {
    const clients = client.listClients();
    expect(Array.isArray(clients)).toBe(true);
    for (const c of clients) {
      expect(typeof c.tty).toBe("string");
      expect(typeof c.sessionName).toBe("string");
      expect(typeof c.pid).toBe("number");
    }
  });

  test("getCurrentSession() returns string or null", () => {
    const session = client.getCurrentSession();
    expect(session === null || typeof session === "string").toBe(true);
  });

  test("getCurrentSession() ignores control-mode clients without a tty", () => {
    class ControlModeAwareClient extends TmuxClient {
      override run(args: readonly string[]) {
        if (args[0] === "list-clients") {
          return {
            args: ["tmux", ...args],
            exitCode: 0,
            stdout: [
              "client-1\t\t100\talpha\t80\t24",
              "/dev/ttys004\t/dev/ttys004\t101\tbeta\t160\t40",
            ].join("\n"),
            stderr: "",
            ok: true,
          };
        }
        return {
          args: ["tmux", ...args],
          exitCode: 0,
          stdout: "",
          stderr: "",
          ok: true,
        };
      }
    }

    const custom = new ControlModeAwareClient();
    expect(custom.getCurrentSession()).toBe("beta");
  });

  test("getClientTty() returns string", () => {
    const tty = client.getClientTty();
    expect(typeof tty).toBe("string");
  });

  test("display() queries tmux format", () => {
    const pid = client.display("#{pid}");
    // tmux server PID should be a number string
    expect(typeof pid).toBe("string");
    if (pid) expect(parseInt(pid, 10)).toBeGreaterThan(0);
  });

  test("getPaneCount() returns number >= 0", () => {
    const count = client.getPaneCount("nonexistent-session-xyz");
    expect(typeof count).toBe("number");
    expect(count).toBeGreaterThanOrEqual(0);
  });

  test("getAllPaneCounts() returns Map", () => {
    const counts = client.getAllPaneCounts();
    expect(counts).toBeInstanceOf(Map);
    for (const [name, count] of counts) {
      expect(typeof name).toBe("string");
      expect(typeof count).toBe("number");
      expect(count).toBeGreaterThan(0);
    }
  });

  test("getGlobalEnv() returns null for missing var", () => {
    const val = client.getGlobalEnv("OPENSESSIONS_TEST_NONEXISTENT_VAR_XYZ");
    expect(val).toBeNull();
  });

  test("getCurrentWindowId() returns string", () => {
    const wid = client.getCurrentWindowId();
    expect(typeof wid).toBe("string");
    // May be empty if not inside tmux, or @N if inside
  });

  test("custom bin option is respected in args", () => {
    const custom = tmux({ bin: "/usr/local/bin/tmux" });
    const result = custom.run(["list-sessions"]);
    expect(result.args[0]).toBe("/usr/local/bin/tmux");
  });

  test("socket options are included in args", () => {
    const custom = tmux({ socketName: "test-socket" });
    const result = custom.run(["list-sessions"]);
    expect(result.args).toContain("-L");
    expect(result.args).toContain("test-socket");
  });

  test("splitWindow can request a full-window split for sidebars", () => {
    const custom = new TestTmuxClient();
    const pane = custom.splitWindow({
      target: "%1",
      direction: "horizontal",
      before: true,
      fullWindow: true,
      size: 26,
      command: "echo sidebar",
    });

    expect(custom.calls[0]).toEqual([
      "split-window",
      "-hb",
      "-f",
      "-l",
      "26",
      "-t",
      "%1",
      "-P",
      "-F",
      "#{pane_id}\t#{session_name}\t#{window_id}\t#{window_index}\t#{pane_index}\t#{pane_active}\t#{pane_tty}\t#{pane_pid}\t#{pane_current_path}\t#{pane_current_command}\t#{pane_title}\t#{pane_width}\t#{pane_height}\t#{pane_left}\t#{pane_right}",
      "echo sidebar",
    ]);
    expect(pane?.id).toBe("%9");
  });

  test("pane terminal reset and history clear use pane-scoped tmux commands", () => {
    const custom = new TestTmuxClient();

    custom.resetPane("%9");
    custom.clearPaneHistory("%9");

    expect(custom.calls[0]).toEqual(["send-keys", "-R", "-t", "%9"]);
    expect(custom.calls[1]).toEqual(["clear-history", "-t", "%9"]);
  });
});
