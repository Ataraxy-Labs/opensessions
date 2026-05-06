import { afterEach, describe, expect, test } from "bun:test";
import { mkdirSync, rmSync, writeFileSync } from "fs";
import { join } from "path";
import { tmpdir } from "os";
import {
  findOtherLiveOpensessionsPids,
  isLastLiveOpensessionsInstance,
  isProcessAlive,
} from "../src/server/server-instance-scope";

const tempDirs: string[] = [];

afterEach(() => {
  for (const dir of tempDirs.splice(0)) {
    rmSync(dir, { recursive: true, force: true });
  }
});

function makeTempDir(): string {
  const dir = join(tmpdir(), `opensessions-scope-${Date.now()}-${Math.random().toString(16).slice(2)}`);
  mkdirSync(dir, { recursive: true });
  tempDirs.push(dir);
  return dir;
}

describe("isProcessAlive", () => {
  test("returns true for the current process", () => {
    expect(isProcessAlive(process.pid)).toBe(true);
  });

  test("returns false for an obviously dead pid", () => {
    // PID 0 is invalid as a target for kill(0); pid 99999999 is almost certainly free.
    expect(isProcessAlive(0)).toBe(false);
    expect(isProcessAlive(99_999_999)).toBe(false);
  });

  test("returns false for non-positive / non-integer inputs", () => {
    expect(isProcessAlive(-1)).toBe(false);
    expect(isProcessAlive(Number.NaN)).toBe(false);
    expect(isProcessAlive(1.5)).toBe(false);
  });
});

describe("findOtherLiveOpensessionsPids", () => {
  test("returns empty when no sibling pid files exist", () => {
    const dir = makeTempDir();
    const ownPidFile = join(dir, "opensessions.pid");
    writeFileSync(ownPidFile, String(process.pid));

    const result = findOtherLiveOpensessionsPids({
      ownPidFile,
      ownPid: process.pid,
      pidDir: dir,
    });
    expect(result).toEqual([]);
  });

  test("excludes the caller's own pid file even if other live pids are listed", () => {
    const dir = makeTempDir();
    const ownPidFile = join(dir, "opensessions.pid");
    writeFileSync(ownPidFile, String(process.pid));

    // The own pid file lists this process — but we must exclude ourselves.
    const result = findOtherLiveOpensessionsPids({
      ownPidFile,
      ownPid: process.pid,
      pidDir: dir,
    });
    expect(result).toEqual([]);
  });

  test("returns live sibling pids while ignoring stale ones", () => {
    const dir = makeTempDir();
    const ownPidFile = join(dir, "opensessions.pid");
    writeFileSync(ownPidFile, String(process.pid));

    writeFileSync(join(dir, "opensessions.alpha.pid"), "111");  // stale
    writeFileSync(join(dir, "opensessions.beta.pid"), "222");   // live (mocked)
    writeFileSync(join(dir, "opensessions.gamma.pid"), "333");  // stale

    const liveSet = new Set([222]);
    const result = findOtherLiveOpensessionsPids({
      ownPidFile,
      ownPid: process.pid,
      pidDir: dir,
      isAlive: (pid) => liveSet.has(pid),
    });
    expect(result).toEqual([222]);
  });

  test("ignores files that don't match the opensessions pid pattern", () => {
    const dir = makeTempDir();
    const ownPidFile = join(dir, "opensessions.pid");
    writeFileSync(ownPidFile, String(process.pid));
    writeFileSync(join(dir, "other.pid"), "444");
    writeFileSync(join(dir, "opensessionsbad.pid"), "555");
    writeFileSync(join(dir, "opensessions.foo.bar.pid"), "666");

    const result = findOtherLiveOpensessionsPids({
      ownPidFile,
      ownPid: process.pid,
      pidDir: dir,
      isAlive: () => true,
    });
    expect(result).toEqual([]);
  });

  test("ignores garbage pid file contents", () => {
    const dir = makeTempDir();
    const ownPidFile = join(dir, "opensessions.pid");
    writeFileSync(ownPidFile, String(process.pid));
    writeFileSync(join(dir, "opensessions.garbage.pid"), "not-a-number");
    writeFileSync(join(dir, "opensessions.empty.pid"), "");
    writeFileSync(join(dir, "opensessions.negative.pid"), "-7");

    const result = findOtherLiveOpensessionsPids({
      ownPidFile,
      ownPid: process.pid,
      pidDir: dir,
      isAlive: () => true,
    });
    expect(result).toEqual([]);
  });

  test("excludes a sibling file that happens to list the caller's pid", () => {
    const dir = makeTempDir();
    const ownPidFile = join(dir, "opensessions.pid");
    writeFileSync(ownPidFile, String(process.pid));
    // Stale sibling file pointing at this process — must not count as another server.
    writeFileSync(join(dir, "opensessions.dup.pid"), String(process.pid));

    const result = findOtherLiveOpensessionsPids({
      ownPidFile,
      ownPid: process.pid,
      pidDir: dir,
      isAlive: () => true,
    });
    expect(result).toEqual([]);
  });

  test("returns empty when the pid directory is missing", () => {
    const dir = join(tmpdir(), `opensessions-scope-missing-${Date.now()}-${Math.random().toString(16).slice(2)}`);
    const ownPidFile = join(dir, "opensessions.pid");
    const result = findOtherLiveOpensessionsPids({
      ownPidFile,
      ownPid: process.pid,
      pidDir: dir,
    });
    expect(result).toEqual([]);
  });
});

describe("isLastLiveOpensessionsInstance", () => {
  test("true when no other live siblings exist", () => {
    const dir = makeTempDir();
    const ownPidFile = join(dir, "opensessions.pid");
    writeFileSync(ownPidFile, String(process.pid));
    writeFileSync(join(dir, "opensessions.stale.pid"), "111");

    expect(isLastLiveOpensessionsInstance({
      ownPidFile,
      ownPid: process.pid,
      pidDir: dir,
      isAlive: () => false,
    })).toBe(true);
  });

  test("false when a live sibling exists", () => {
    const dir = makeTempDir();
    const ownPidFile = join(dir, "opensessions.17000.pid");
    writeFileSync(ownPidFile, String(process.pid));
    writeFileSync(join(dir, "opensessions.pid"), "9999");

    expect(isLastLiveOpensessionsInstance({
      ownPidFile,
      ownPid: process.pid,
      pidDir: dir,
      isAlive: (pid) => pid === 9999,
    })).toBe(false);
  });
});
