# Fix even-horizontal shell helpers tests - 2026-05-05

## Summary

Fixed 5 pre-existing failing tests in `packages/runtime/test/even-horizontal-layout.test.ts`
that were broken by an interaction between `sh -lc` (login shell) and the local user's
shell profile printing a JDK warning to stdout.

---

## Root cause

`runHelper()` invoked the shell as `sh -lc`. The `-l` flag makes it a login shell, which
sources the user's profile (`.profile` / `.bashrc` / `.zprofile`). On Indeed-managed
machines, the profile prints a JDK version warning to **stdout**:

```
JDK version 17.0.16 is not the current Indeed default and should be updated to 17.0.18.

See https://wiki.indeed.com/x/27-ADw for details on how to correct this problem.
```

That warning was prepended to every helper's captured stdout, so equality checks like
`toBe("1")` and `toBe("left")` failed. Each test also took ~300 ms, almost all of which
was profile-load overhead.

The helper script (`integrations/tmux-plugin/scripts/even-horizontal-common.sh`) is a
plain POSIX shell library that defines pure-text functions — it has no need for a login
shell.

## Fix

Dropped the `-l` flag in `runHelper()`. Tests now pass and run in ~25 ms each.

## Verification

```
cd packages/runtime && bun test test/even-horizontal-layout.test.ts
```
→ 5 pass, 0 fail.

```
cd packages/runtime && bun test
```
→ 367 pass, 0 fail across 23 files.

---

## Files Changed

- `packages/runtime/test/even-horizontal-layout.test.ts` — change `sh -lc` to `sh -c`
  in `runHelper()` so user-profile output does not contaminate captured stdout.

---

## Notes for future sessions

- This is a test-environment fix, not a behavior change. The shell helpers themselves
  were always correct.
- If similar `Bun.spawnSync(["sh", "-lc", ...])` patterns appear elsewhere in the test
  suite, they're at risk of the same contamination on machines with chatty shell
  profiles. Prefer `sh -c` for tests that capture stdout.
