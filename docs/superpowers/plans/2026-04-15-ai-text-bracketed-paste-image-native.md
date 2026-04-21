# AI Text Paste via Bracketed Paste, Image Paste via Native Shortcut

## Goal

Fix AI-pane paste behavior in Mini-Term with the correct split strategy:

- **plain text** in AI panes should use **bracketed paste injection**
- **images** in AI panes should keep the current **native image shortcut path**

This is required because the current text implementation using only `Ctrl+V` (`\x16`) is not equivalent to real terminal paste semantics in the embedded terminal environment.

---

## Current Problems

### Problem A - text paste still does not behave correctly

Current AI text paste path uses:

- `sendAiTextPasteShortcut()`
- which currently sends `\x16`

This is only a control character / key simulation.
It is not a reliable replacement for real paste-data delivery in embedded xterm/Tauri.

Observed behavior:

- sometimes no response
- pasted-block behavior differs from native terminal
- text paste may not trigger the expected Claude Code UI reliably

### Problem B - image paste should remain as-is

The current image path has improved and should not be regressed.

In particular on Windows AI panes:

- image paste should continue using the image-native shortcut path (currently `Alt+V`)

---

## Root Cause

### Text path is using a key simulation, not actual paste data

For AI panes, plain text is currently handled by sending `\x16`.

That means Mini-Term is effectively saying:

- "the user pressed Ctrl+V"

instead of:

- "here is a bracketed pasted text block"

In embedded terminal environments, this difference matters.
Claude Code / Codex / Gemini are much more likely to recognize a real pasted block when the terminal receives **bracketed paste sequences**.

---

## Required Fix

## Main rule

For **AI panes**:

### Plain text
Use **bracketed paste injection**, not keyboard shortcut simulation.

### Raw image
Keep the existing image-native shortcut path.

That means:

- text and image must be handled by two different mechanisms
- do not try to unify them behind a single `Ctrl+V` path

---

## Exact Text Strategy

### Required bracketed paste format

Wrap clipboard text as:

```text
ESC [ 200 ~
<clipboard text>
ESC [ 201 ~
```

In string form:

```ts
`\x1b[200~${text}\x1b[201~`
```

### Important requirement

For AI-pane plain text:

- do not call `sendAiTextPasteShortcut()` if it only sends `\x16`
- instead directly write the bracketed-paste wrapped text into PTY

Example direction:

```ts
await enqueuePtyWrite(ptyId, `\x1b[200~${text}\x1b[201~`);
```

---

## Exact Image Strategy

### Do not change image-native handling

For AI-pane image paste:

- keep the current image-specific shortcut path
- on Windows, continue using the current image-native shortcut (`Alt+V`) if that is what currently works

### Important constraint

Do not use bracketed paste for images.
Bracketed paste is for text blocks only.

---

## Files to Modify

### 1. `src/utils/terminalCache.ts`

Primary target.

Update:

- `sendAiTextPasteShortcut()`
- or replace it with a text-specific bracketed paste helper
- `pasteToTerminal()` AI-pane branch
- comments explaining text/image split

### 2. `src/components/TerminalInstance.tsx`

Usually no logic change needed.
It can continue calling:

- `pasteToTerminal(ptyId)`

---

## Recommended Refactor

### Replace current text helper

Instead of a helper that sends `\x16`, create something like:

```ts
async function sendAiBracketedTextPaste(ptyId: number, text: string): Promise<void>
```

Behavior:

- wrap text in bracketed-paste markers
- send it with `enqueuePtyWrite`

### Keep image helper separate

Retain something like:

```ts
async function sendAiImagePasteShortcut(ptyId: number): Promise<void>
```

Behavior:

- Windows -> `Alt+V`
- macOS/Linux -> existing native behavior

---

## AI-Pane Clipboard Decision Rules

### If clipboard is `raw-image`
- use image-native shortcut path
- do not downgrade to text

### If clipboard is `plain-text`
- use bracketed paste injection
- do not use `Ctrl+V` key simulation
- do not directly inject raw text without bracketed markers

### If clipboard is `rich-object / unknown`
- keep current conservative fallback behavior
- but do not let this path break image handling

---

## Anti-Patterns to Avoid

Do **not** implement any of these:

- plain text -> `\x16`
- plain text -> raw PTY injection without bracketed paste markers
- images -> bracketed paste
- one shared helper for text and image
- sending both bracketed paste and image shortcut in the same path

---

## Comments That Must Be Added

Near the AI text paste helper, explain:

- `Ctrl+V` key simulation is not reliable enough in embedded Mini-Term/xterm for AI text paste
- AI CLIs need actual pasted-block semantics
- bracketed paste is the correct text mechanism here
- image paste remains a separate native-shortcut path

---

## Validation Scenarios

After implementing, verify:

### A. Windows + Claude Code plain text

Paste a multiline text block in an AI pane.

Expected:

- behavior is closer to native terminal paste than the current `\x16` simulation
- pasted-block UI appears reliably
- large pasted text behaves more like native terminal behavior

### B. Windows + Claude Code image paste

Paste an image in an AI pane.

Expected:

- image paste still works via native image path
- image does not degrade into path/text behavior

### C. Windows + Codex / Gemini plain text

Expected:

- multiline text paste works as a pasted block rather than key simulation

### D. Non-AI pane regression

Expected:

- existing non-AI text paste behavior remains intact
- existing non-AI image/path behavior remains intact

---

## Acceptance Criteria

- [ ] AI-pane plain-text paste no longer uses `\x16` shortcut simulation
- [ ] AI-pane plain-text paste uses bracketed paste injection
- [ ] AI-pane image paste still uses its existing native image shortcut path
- [ ] text and image helpers are clearly separated
- [ ] non-AI paste behavior does not regress
- [ ] comments explain why text uses bracketed paste and image does not

---

## Instruction for Claude / Codex

Implement exactly this behavior:

- in `src/utils/terminalCache.ts`, replace AI plain-text paste key simulation with bracketed paste text injection
- keep AI image paste on its current native image shortcut path
- do not merge text and image strategies
- do not use `Ctrl+V` simulation as the final AI text paste mechanism
- add comments explaining why bracketed paste is required for AI text and why image paste must remain separate
