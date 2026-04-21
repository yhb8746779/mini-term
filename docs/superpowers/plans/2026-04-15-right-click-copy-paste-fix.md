# Right-Click Copy / Paste Fix

## Two separate bugs

---

## Bug 1: Right-click copy — selection disappears before copy runs

### Symptom

User selects text in terminal → right-clicks → the selected text is NOT copied.
Instead, the selection is gone and paste fires.

### Root cause

`TerminalInstance.tsx` → `handleContextMenu`:

```ts
const handleContextMenu = (e: React.MouseEvent) => {
  e.preventDefault();
  const selectedText = getAnySelectedText(ptyId);  // ← too late
  ...
```

When Claude/Codex TUI runs inside xterm.js, mouse mode is active.
On Windows, the right-click `mousedown` event causes xterm.js to clear its
internal selection **before** the `contextmenu` event fires.

So `getAnySelectedText()` returns empty at contextmenu time, the code
sees no selection, and calls `pasteToTerminal()` instead of copy.

### Fix

Snapshot the selection on `mousedown` (right-click button = 2), store in a
`useRef`. Use the snapshot in `handleContextMenu`.

**In `TerminalInstance.tsx`**:

```tsx
// add ref near other refs
const selectionSnapshot = useRef('');

// add handler
const handleMouseDown = (e: React.MouseEvent<HTMLDivElement>) => {
  if (e.button === 2) {
    selectionSnapshot.current = getAnySelectedText(ptyId);
  }
};

// modify handleContextMenu — replace the live call with snapshot
const handleContextMenu = (e: React.MouseEvent<HTMLDivElement>) => {
  e.preventDefault();
  const selectedText = selectionSnapshot.current;  // use snapshot
  selectionSnapshot.current = '';                   // clear after read

  if (_isMacOS || _isWindows) {
    if (selectedText) {
      void copyTextToClipboard(selectedText);
      getCachedTerminal(ptyId)?.term.clearSelection();
    } else {
      void pasteToTerminal(ptyId).finally(() => {
        getCachedTerminal(ptyId)?.term.focus();
      });
    }
    return;
  }

  // Linux menu path: also update to use selectionSnapshot
  ...
};
```

Add `onMouseDown={handleMouseDown}` to the terminal container div
(the same div that already has `onContextMenu={handleContextMenu}`).

---

## Bug 2: AI pane text paste — no response

### Symptom

Clipboard has plain text → right-click paste in AI pane → nothing happens.

Sometimes: WeChat screenshot taken → then text copied → paste →
Claude/Codex shows "no image" error.

### Root cause

In `terminalCache.ts` → `detectClipboardPayload()`, the fallback chain is:

1. `navigator.clipboard.read()` — may throw (Tauri WebView permission)
2. **`readImage()`** — tries to read image from clipboard
3. `readText()` — reads text

**Problem A**: When step 1 throws, step 2 runs `readImage()`.
If the clipboard previously had a WeChat screenshot and now has text,
Tauri's `readImage()` may still succeed (stale OS clipboard cache).
This returns `raw-image` when the clipboard actually has text.

**Problem B**: When `plain-text` IS correctly detected, the current AI pane
code calls `cache.get(ptyId)?.term.paste(clipboard.text)`.
`term.paste()` exists in xterm.js 6 and is correct in principle,
but it needs the terminal to be focused to work reliably.
If focus has shifted (e.g. right-click stole focus), paste is silent.

### Fix

**Fix A — swap fallback order in `detectClipboardPayload()`**

Move `readText()` **before** `readImage()` in the fallback chain.
Text is far more common than image paste; this ensures fresh text
clipboard is detected correctly even if the OS previously held an image.

```ts
// After navigator.clipboard.read() fails:

// 1. Try text first
try {
  const text = await readText();
  if (text && text.trim()) return { kind: 'plain-text', text };
} catch { /* ignore */ }

// 2. Then image
try {
  const image = await readImage();
  await image.size();
  return { kind: 'raw-image' };
} catch { /* non-image */ }
```

**Fix B — ensure focus before `term.paste()` in AI pane**

```ts
// AI pane plain-text path:
const entry = cache.get(ptyId);
if (entry) {
  entry.term.focus();          // ensure focused first
  entry.term.paste(clipboard.text);
}
```

---

## Files to change

| File | Change |
|------|--------|
| `src/components/TerminalInstance.tsx` | Add `selectionSnapshot` ref + `handleMouseDown`; use snapshot in `handleContextMenu`; add `onMouseDown` to container div |
| `src/utils/terminalCache.ts` | Swap `readText()` before `readImage()` in fallback chain; add `entry.term.focus()` before `term.paste()` in AI pane branch |

## Do NOT change

- The AI pane image path (`raw-image` → `sendPlatformAiPasteShortcut`) — correct as-is
- Non-AI pane behavior — keep unchanged
- `clearSelection()` after copy — keep as-is

## Validation

1. Select text in Claude/Codex pane → right-click → text is copied, selection clears
2. No selection → right-click → paste fires correctly  
3. WeChat screenshot → copy text → AI pane right-click paste → text pastes (not "no image")
4. WeChat screenshot → (no text copy) → AI pane right-click paste → image path pastes
5. Non-AI pane behavior unchanged
