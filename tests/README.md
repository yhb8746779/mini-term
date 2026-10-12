# Terminal clipboard regression tests

Run `npm ci`, `npx playwright install chromium webkit`, then `npm run test:terminal`.

The browser tests load the real TerminalInstance, xterm, clipboard classifier and
paste router. Only Tauri IPC and the operating-system clipboard are mocked.
They cover local macOS Codex image shortcuts, Finder and WeChat clipboard formats,
Claude's existing local path behavior, SSH PNG normalization and bridge/fallback
paths, bracketed text paste, and right-click selection under mouse reporting.
They also cover bridge profile copying, independent editing, persistence of the
selection, quick switching and deletion, and selection of the profile sent to IPC.

Windows regression cases additionally cover selected/unselected right-click with and
without mouse reporting, failed copy, Control-left click, and unchanged local/SSH
Alt+V image shortcuts. The release workflow runs these cases on a Windows runner
with Chromium; the native clipboard IPC remains mocked.

Native manual verification is still required for WeChat's real pasteboard formats
and the remote sandbox bridge. In the Mac test app:

1. Start a fresh Codex session with inline mode enabled.
2. Copy a WeChat image and right-click the input; check for an image attachment.
3. Select conversation text and right-click; check that it copies without pasting.
4. Right-click with no selection; check that text pastes once.
5. Connect to the existing SSH sandbox and repeat screenshot/Finder image paste in
   Claude Code; check that its native image attachment still appears.
6. In Settings → SSH image bridge, copy the current profile and edit the copy for
   another sandbox. Use the toolbar selector to switch before the next image paste.

No user PTY, SSH session or system clipboard is accessed by the automated tests.
