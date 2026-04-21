# AI Pane Text Paste Recovery

## Goal

Restore native text paste behavior for Claude Code / Codex / Gemini CLI running inside Mini-Term AI panes, **without regressing image paste convenience**.

Current Mini-Term behavior incorrectly degrades **plain-text paste** in AI panes into ordinary PTY input, which causes:

- no `[Pasted text #1 +N lines]` preview
- no large-paste safety prompt (for example 5KB-style warnings)
- input area showing only the first line or partial head content
- a visual impression that content may have been lost

At the same time, current image paste behavior in AI panes is useful and should be preserved.

---

## Root Cause

Current AI-pane paste logic in `src/utils/terminalCache.ts` does this:

- detect AI pane
- if clipboard is plain text
  - directly call `enqueuePtyWrite(ptyId, clipboard.text)`
- only non-text / rich / unknown clipboard payloads use platform paste shortcut

This breaks **text paste semantics** for AI CLIs, because the TUI no longer knows the content came from a native paste event.

The image path / native paste path should not be regressed while fixing this.

---

## Required Fix

### Main rule

For **AI panes**, change only the **plain-text** branch.

#### Required behavior for AI panes

- `plain-text`
  - **must use platform native paste shortcut**
  - **must not** directly write clipboard text into PTY
- `raw-image`
  - keep the current working image/native paste behavior
- `rich-object / unknown`
  - keep the current native/fallback strategy unless a specific regression is found

### Important scope control

This task is about recovering **native text paste semantics**.
It is **not** a request to redesign or remove image paste handling.

---

## Files to Modify

### 1. `src/utils/terminalCache.ts`

Primary target.

Update:

- `pasteToTerminal()`
- related comments around AI text paste strategy

### 2. `src/components/TerminalInstance.tsx`

Usually no major logic change required.
It should continue to call:

- `pasteToTerminal(ptyId)`

---

## Exact Logic Change

## Existing incorrect logic (AI pane)

Current AI-pane branch effectively behaves like this:

```ts
if (isAiPane) {
  if (clipboard.kind === 'plain-text' && clipboard.text) {
    await enqueuePtyWrite(ptyId, clipboard.text);
    return;
  }
  await sendPlatformAiPasteShortcut(ptyId);
  return;
}
```

## Required logic

Replace only the plain-text handling so that AI-pane text paste goes through native paste shortcut.

Recommended form:

```ts
if (isAiPane) {
  if (clipboard.kind === 'plain-text' && clipboard.text) {
    await sendPlatformAiPasteShortcut(ptyId);
    return;
  }

  // keep existing image / rich / unknown behavior
  // do not regress current image paste convenience
  ...
}
```

### Acceptable implementation detail

If the current AI-pane handling for `raw-image` already works well via native paste shortcut, it may remain unchanged.

The key requirement is:

- **plain text in AI panes must no longer use direct PTY injection**

---

## Platform Strategy

### Windows

Continue using the existing Windows AI native paste shortcut already used by Mini-Term.

If the project currently uses:

- `Alt+V`

for AI native paste, keep using that unless there is a proven reason to change it.

### macOS / Linux

Keep the existing native shortcut behavior already used in the project.

---

## Important Constraints

### Do NOT do this

Do not implement:

- direct PTY write for small text, native paste for large text
- hybrid behavior like "send text first, then shortcut"
- logic that removes or degrades image paste convenience
- duplicate fallback chains that may paste twice

### Preferred v1 scope

For AI panes:

- fix **plain-text** paste to use native shortcut
- keep image paste behavior as convenient as it is now

---

## Documentation Requirement

Add or update comments near the AI-pane branch in `pasteToTerminal()` explaining:

- Claude/Codex/Gemini CLI rely on native paste semantics for text paste block recognition
- direct PTY text injection loses text-paste metadata
- image paste behavior should remain intact and must not be regressed by this fix

---

## Validation Scenarios

After implementing, verify at minimum:

### Windows + Claude Code text paste

Paste a multiline plain-text block in an AI pane.

Expected:

- Claude Code shows native pasted block behavior
- `[Pasted text #1 +N lines]` appears again when appropriate
- large-paste warning / preview logic is restored
- the UI no longer looks like only the first sentence was pasted

### Windows + Claude Code image paste

Right-click paste an image in an AI pane.

Expected:

- image paste still works conveniently
- image workflow is not degraded by the text-paste fix

### Windows + Codex / Gemini text paste

Paste multiline plain text in an AI pane.

Expected:

- text paste uses native path rather than ordinary PTY typing semantics

### Non-AI pane regression check

Expected:

- non-AI plain-text paste still works
- image-path workflows still work as before

---

## Acceptance Criteria

- [ ] AI panes no longer directly send **plain text** via `enqueuePtyWrite()`
- [ ] AI-pane plain-text paste uses native paste shortcut
- [ ] Windows Claude Code regains native pasted-block behavior for text
- [ ] existing image paste convenience is preserved
- [ ] no accidental double-paste is introduced
- [ ] non-AI pane paste behavior does not regress
- [ ] comments explain why text paste was changed and why image paste must stay intact

---

## Instruction for Claude / Codex

Implement exactly this behavior:

- in `src/utils/terminalCache.ts`, change `pasteToTerminal()` so that **plain-text paste in AI panes** uses native paste shortcut instead of direct PTY text injection
- do **not** redesign or regress AI image paste behavior
- keep non-AI pane behavior unchanged unless required
- add comments explaining why this is necessary for Claude/Codex/Gemini text-paste block recognition and why image paste convenience must be preserved
