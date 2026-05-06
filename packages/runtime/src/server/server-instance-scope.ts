/**
 * Helpers for scoping a server instance's blast radius on shutdown.
 *
 * The runtime intentionally allows multiple opensessions servers to coexist on
 * the same machine — typically one per tmux socket (see {@link resolveServerKey}
 * / {@link resolvePidFile} in shared.ts). When one server shuts down it must
 * not tear down resources owned by its siblings:
 *   - sidebar panes spawned by another server
 *   - global tmux hooks another server still depends on
 *
 * This module's job is the second concern: detect whether *other* live
 * opensessions servers exist on the machine so the caller can decide whether
 * it's safe to unregister global tmux hooks.
 */
import { readdirSync, readFileSync } from "fs";
import { dirname, resolve } from "path";

const PID_FILE_RE = /^opensessions(\.[^.]+)?\.pid$/;

/** True iff a process with `pid` is currently alive. */
export function isProcessAlive(pid: number): boolean {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  try {
    // Signal 0 doesn't deliver a signal; it just probes existence + permission.
    process.kill(pid, 0);
    return true;
  } catch (err: unknown) {
    // ESRCH = no such process. EPERM = process exists but we can't signal it
    // (still alive, owned by another user — counts as live for our purposes).
    if (err && typeof err === "object" && "code" in err) {
      return (err as { code?: string }).code === "EPERM";
    }
    return false;
  }
}

export interface FindOtherLivePidsOptions {
  /** PID file path of the current server (excluded from the scan). */
  ownPidFile: string;
  /** PID of the current server (excluded if it appears in any file). */
  ownPid: number;
  /** Override directory to scan. Defaults to `dirname(ownPidFile)` (typically /tmp). */
  pidDir?: string;
  /** Test seam — defaults to {@link isProcessAlive}. */
  isAlive?: (pid: number) => boolean;
  /** Test seam — defaults to fs readdirSync. */
  readDir?: (dir: string) => string[];
  /** Test seam — defaults to fs readFileSync. */
  readFile?: (path: string) => string;
}

/**
 * Find PIDs of *other* live opensessions servers on this machine, by scanning
 * the PID-file directory for files matching `opensessions[.<key>].pid` and
 * probing each PID for liveness.
 *
 * Stale PID files (process is dead) are silently ignored. The caller's own
 * PID file and own PID are excluded from the result.
 */
export function findOtherLiveOpensessionsPids(
  opts: FindOtherLivePidsOptions,
): number[] {
  const isAlive = opts.isAlive ?? isProcessAlive;
  const readDir = opts.readDir ?? ((d) => readdirSync(d));
  const readFile = opts.readFile ?? ((p) => readFileSync(p, "utf-8"));
  const ownPidFile = resolve(opts.ownPidFile);
  const dir = opts.pidDir ?? dirname(ownPidFile);

  let entries: string[];
  try {
    entries = readDir(dir);
  } catch {
    return [];
  }

  const live: number[] = [];
  for (const entry of entries) {
    if (!PID_FILE_RE.test(entry)) continue;
    const fullPath = resolve(dir, entry);
    if (fullPath === ownPidFile) continue;

    let raw: string;
    try {
      raw = readFile(fullPath);
    } catch {
      continue;
    }
    const pid = Number.parseInt(raw.trim(), 10);
    if (!Number.isInteger(pid) || pid <= 0) continue;
    if (pid === opts.ownPid) continue;
    if (!isAlive(pid)) continue;
    live.push(pid);
  }
  return live;
}

/**
 * Convenience wrapper: returns true iff this server is the only live
 * opensessions instance and is therefore safe to tear down global mux state
 * (e.g. tmux global hooks).
 */
export function isLastLiveOpensessionsInstance(
  opts: FindOtherLivePidsOptions,
): boolean {
  return findOtherLiveOpensessionsPids(opts).length === 0;
}
