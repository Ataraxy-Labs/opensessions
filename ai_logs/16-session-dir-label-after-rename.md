# Session Directory Label After Rename - 2026-05-29

## Summary

Fixed a sidebar display bug where renaming a session to match the working directory basename caused the directory label under the session name to disappear.

---

## Details

The session card previously hid the directory row whenever `basename(session.dir) === session.name` to avoid duplicate labels for sessions created from a directory. After a manual rename, this could hide useful context: for example, renaming `repos` to `Geoalchemist` made a `.../Geoalchemist` working directory stop appearing under the session name.

The fix keeps the compact basename display when it differs from the session name. When the basename equals the session name, the sidebar now falls back to `parent/basename`, preserving directory context without rendering the full path.

---

## Files Changed

- `apps/tui/src/index.tsx` - Added `sessionDirLabel()` and changed session cards to show `parent/basename` instead of hiding matching directory names.
- `packages/runtime/test/close-sidebar.test.ts` - Added regression coverage ensuring matching session/directory names still produce a visible directory label.

---

## Verification

```bash
bun test packages/runtime/test/close-sidebar.test.ts
cd apps/tui && bun run build
```
