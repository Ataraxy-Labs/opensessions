import { describe, test, expect } from "bun:test";
import type { ClientCommand } from "../src/shared";
import { readFileSync } from "fs";
import { resolve } from "path";

describe("close-sidebar command", () => {
  test("ClientCommand union accepts close-sidebar type", () => {
    const cmd: ClientCommand = { type: "close-sidebar" };
    expect(cmd.type).toBe("close-sidebar");
  });

  test("ClientCommand union still accepts the legacy global quit type", () => {
    const cmd: ClientCommand = { type: "quit" };
    expect(cmd.type).toBe("quit");
  });
});

// These tests guard the security-sensitive contract that the TUI's `q`
// keystroke must NOT trigger the global `quit` path (which calls
// process.exit(0) on the server and tears down every sidebar across every
// session). They check the source files directly to avoid spinning up the
// server in tests. See ai_logs/02-harden-tui-input.md for the full context.
describe("server: close-sidebar wiring", () => {
  const serverPath = resolve(__dirname, "../src/server/index.ts");
  const serverSrc = readFileSync(serverPath, "utf-8");

  test("server registers a close-sidebar WS command handler", () => {
    expect(serverSrc).toMatch(/case "close-sidebar":/);
    expect(serverSrc).toMatch(/closeLocalSidebar\(ws\)/);
  });

  test("close-sidebar handler is scoped to one client's pane", () => {
    expect(serverSrc).toMatch(/function closeLocalSidebar\(ws: any\): void/);
    // Must read the per-client paneId map populated via identify-pane.
    expect(serverSrc).toMatch(/clientPaneIds\.get\(ws\)/);
  });

  test("close-sidebar handler does NOT exit the process or touch other sidebars globally", () => {
    const fnMatch = serverSrc.match(/function closeLocalSidebar[\s\S]*?\n  \}\n/);
    expect(fnMatch).not.toBeNull();
    const body = fnMatch![0];
    expect(body).not.toMatch(/process\.exit/);
    // Must not delegate to quitAll, which kills every sidebar pane.
    expect(body).not.toMatch(/quitAll\(/);
  });

  test("identify-pane records paneId so close-sidebar can scope to it", () => {
    expect(serverSrc).toMatch(/clientPaneIds\.set\(ws, cmd\.paneId\)/);
  });

  test("refresh only broadcasts state and does not fan out sidebar spawns", () => {
    const refreshRouteMatch = serverSrc.match(/url\.pathname === "\/refresh"\)[\s\S]*?return new Response\("ok", \{ status: 200 \}\);/);
    expect(refreshRouteMatch).not.toBeNull();
    const refreshRoute = refreshRouteMatch![0];

    expect(refreshRoute).toMatch(/broadcastState\(\)/);
    expect(refreshRoute).not.toMatch(/ensureSidebar|debouncedEnsureSidebar|queueEnsureSidebarAcrossAllWindows/);
    expect(serverSrc).not.toMatch(/function queueEnsureSidebarAcrossAllWindows/);
  });
});

describe("TUI: q keystroke wiring", () => {
  const tuiPath = resolve(__dirname, "../../../apps/tui/src/index.tsx");
  const tuiSrc = readFileSync(tuiPath, "utf-8");

  test("q opens a confirm-quit modal — does NOT immediately send quit", () => {
    // Find the q case in the keyboard switch
    const qCaseMatch = tuiSrc.match(/case "q":[\s\S]*?break;/);
    expect(qCaseMatch).not.toBeNull();
    const qCase = qCaseMatch![0];
    expect(qCase).toMatch(/setModal\("confirm-quit"\)/);
    expect(qCase).not.toMatch(/send\(\{ type: "quit" \}\)/);
    expect(qCase).not.toMatch(/fetch.*\/quit/);
  });

  test("confirm-quit modal accepts ONLY Enter, sends close-sidebar (not quit)", () => {
    const confirmQuitMatch = tuiSrc.match(/if \(currentModal === "confirm-quit"\)[\s\S]*?return;\s*\}/);
    expect(confirmQuitMatch).not.toBeNull();
    const block = confirmQuitMatch![0];
    expect(block).toMatch(/key\.name === "return"/);
    expect(block).toMatch(/send\(\{ type: "close-sidebar" \}\)/);
    expect(block).not.toMatch(/send\(\{ type: "quit" \}\)/);
  });

  test("confirm-kill modal requires Enter, not bare 'y'", () => {
    const confirmKillMatch = tuiSrc.match(/if \(currentModal === "confirm-kill"\)[\s\S]*?return;\s*\}/);
    expect(confirmKillMatch).not.toBeNull();
    const block = confirmKillMatch![0];
    expect(block).toMatch(/key\.name === "return"/);
    expect(block).not.toMatch(/key\.name === "y"/);
  });

  test("destructive shortcuts are gated on paneHasTerminalFocus", () => {
    expect(tuiSrc).toMatch(/if \(!paneHasTerminalFocus\(\)\) return/);
  });

  test("focus-event tracking is wired up via DECSET 1004", () => {
    expect(tuiSrc).toMatch(/\\x1b\[\?1004h/);
    expect(tuiSrc).toMatch(/\\x1b\[\?1004l/);
    expect(tuiSrc).toMatch(/setPaneHasTerminalFocus\(true\)/);
    expect(tuiSrc).toMatch(/setPaneHasTerminalFocus\(false\)/);
  });
});

describe("TUI: sessionizer sidebar restore", () => {
  const tuiPath = resolve(__dirname, "../../../apps/tui/src/index.tsx");
  const tuiSrc = readFileSync(tuiPath, "utf-8");
  const sessionizerPath = resolve(__dirname, "../../../apps/tui/scripts/sessionizer.sh");
  const sessionizerSrc = readFileSync(sessionizerPath, "utf-8");

  test("new-session popup passes server connection details to the sessionizer", () => {
    const createSessionMatch = tuiSrc.match(/function createNewSession\(\)[\s\S]*?\n  \}/);
    expect(createSessionMatch).not.toBeNull();
    const block = createSessionMatch![0];
    expect(block).toMatch(/OPENSESSIONS_HOST: SERVER_HOST/);
    expect(block).toMatch(/OPENSESSIONS_PORT: String\(SERVER_PORT\)/);
    expect(block).toMatch(/OPENSESSIONS_TOKEN_FILE: TOKEN_FILE/);
  });

  test("sessionizer asks the server to refresh and ensure the sidebar after switching", () => {
    expect(sessionizerSrc).toMatch(/notify_opensessions\(\)/);
    expect(sessionizerSrc).toMatch(/\/refresh/);
    expect(sessionizerSrc).toMatch(/\/ensure-sidebar/);
    expect(sessionizerSrc).toMatch(/notify_opensessions "\$session_name"/);
  });

  test("sessionizer prefers a typed valid directory over fzf's highlighted match", () => {
    const queryCheck = sessionizerSrc.indexOf('[ -n "$query" ] && [ -d "$query" ]');
    const matchCheck = sessionizerSrc.indexOf('[ -n "$match" ]');

    expect(queryCheck).toBeGreaterThan(-1);
    expect(matchCheck).toBeGreaterThan(-1);
    expect(queryCheck).toBeLessThan(matchCheck);
  });
});

describe("tmux provider: focus-events forwarding", () => {
  const providerPath = resolve(__dirname, "../../mux/providers/tmux/src/provider.ts");
  const providerSrc = readFileSync(providerPath, "utf-8");

  test("setupHooks enables tmux focus-events option globally", () => {
    expect(providerSrc).toMatch(/set-option.*focus-events.*on/);
  });
});
