# AI Pane Paste Recovery - Text and Image Follow-up

## Goal

Fix the remaining AI-pane paste issues in Mini-Term, with two separate requirements:

1. **Text paste in AI panes** should behave as close as possible to the native terminal experience used by Claude Code / Codex / Gemini CLI.
2. **Image paste in AI panes** must not regress into text/path paste behavior.

This is a follow-up task after the first paste patch.

---

## Current Problems

### Problem A: text paste is only partially fixed

Current implementation changed AI text paste from direct PTY text injection to sending `Ctrl+V` (`\x16`).

This is better than direct PTY injection, but still does **not fully match native terminal paste behavior**.

Observed result:

- Claude Code now shows some `[Pasted text #N +M lines]` behavior
- but the presentation still differs from the native terminal
- large-text handling is still not fully equivalent to the original native behavior

### Problem B: image paste regressed

Current implementation may classify clipboard image content as plain text too early, causing image paste to become text/path-like behavior instead of native AI image paste behavior.

Observed result:

- image paste no longer appears as the AI-native image block style
- image handling may degrade into path/text behavior

---

## Root Cause Analysis

### Root cause 1: text paste still uses keyboard simulation, not true native paste semantics

Current AI text paste path in `src/utils/terminalCache.ts`:

- `plain-text` in AI pane -> `sendAiTextPasteShortcut()`
- `sendAiTextPasteShortcut()` currently sends `\x16`

This is only a simulated `Ctrl+V` keystroke.

It is **not guaranteed to be equivalent** to the native terminal paste pipeline that Claude Code expects for full pasted-block behavior.

### Root cause 2: clipboard detection order favors text too early

Current fallback order in `detectClipboardPayload()` effectively becomes:

1. `navigator.clipboard.read()`
2. `readText()`
3. `readImage()`

This means that if `navigator.clipboard.read()` fails to expose image metadata clearly, but `readText()` returns some text representation, the clipboard may be classified as `plain-text` before image detection is attempted.

That is exactly the wrong priority for AI-image paste preservation.

---

## Required Fixes

## Part 1 - Text Paste in AI Panes

### Requirement

For **AI-pane text paste**, do not stop at a naive `Ctrl+V` key simulation if that still differs from native behavior.

The implementation should move toward the most native-equivalent paste path Mini-Term can provide for Claude Code / Codex / Gemini CLI.

### Immediate v2 requirement

Keep the AI text path isolated and explicitly documented.

If `Ctrl+V` simulation remains the only available practical mechanism inside Mini-Term, then:

- keep it as a transitional implementation
- but clearly structure the code so text paste strategy is separated from image paste strategy
- and do not let image handling be affected by text fallback heuristics

### Important note

This task does **not** require solving perfect native parity for text paste in one step if the terminal/WebView environment does not expose a better mechanism.

However, the code should:

- make the text strategy explicit
- avoid mixing text and image logic
- stop image paste from being broken by text-first fallback ordering

---

## Part 2 - Image Paste in AI Panes

### Requirement

For **AI-pane image paste**, restore the previous convenient behavior.

Image paste must not be reclassified as plain text before image detection has had a fair chance to succeed.

### Required behavior

In AI panes:

- if clipboard really contains an image, prefer the image-native path
- do not let `readText()` take precedence over image detection in a way that turns images into text/path behavior

### Key rule

For AI panes, image detection must be prioritized over plain-text fallback.

---

## Exact Implementation Guidance

## File: `src/utils/terminalCache.ts`

This is the main file to update.

### 1. Split AI text strategy and AI image strategy clearly

Keep two clearly separated helpers, for example:

- `sendAiTextPasteShortcut(...)`
- `sendAiImagePasteShortcut(...)`

But make sure their usage is driven by correct clipboard classification.

### 2. Fix clipboard detection priority for AI image scenarios

Current logic favors `readText()` before `readImage()` in fallback.

That is the likely reason image paste regressed.

#### Required adjustment

For AI-pane paste decision-making, do **not** let plain-text fallback win before image fallback is checked.

In practice, one of the following approaches is acceptable:

### Option A (preferred)

Keep `detectClipboardPayload()` generic, but in the AI-pane path:

- attempt to preserve image classification first
- if clipboard API says image -> image path
- if clipboard API is inconclusive, try image fallback before treating it as plain text for AI-pane handling

### Option B

Split detection into two modes:

- generic detection for non-AI panes
- AI-aware detection for AI panes

Where AI-aware detection prefers:

1. image
2. rich object / unknown
3. plain text

This is also acceptable if cleaner.

### 3. Do not break non-AI pane behavior

For non-AI panes, existing text/path/image behavior can remain as-is unless strictly necessary.

This follow-up task is focused on AI panes.

---

## Suggested AI-pane Decision Rules

### AI pane clipboard handling should behave like this:

#### If clipboard is image
- use AI image-native shortcut path
- Windows: keep image-specific shortcut behavior
- do not downgrade image into plain text/path behavior

#### If clipboard is plain text
- use AI text-native shortcut strategy
- do not directly `enqueuePtyWrite(clipboard.text)`

#### If clipboard is rich/unknown
- prefer text-native shortcut fallback rather than direct PTY write

---

## Anti-Patterns to Avoid

Do **not** implement any of the following:

- text-first fallback before image detection in AI panes
- direct PTY text injection for AI-pane plain text
- one combined fallback path that treats text and image equally
- sending both text and shortcut together
- trying to "fix" text paste by breaking image paste

---

## Comments That Must Be Added

Near the AI-pane paste branch, add comments explaining:

- text paste and image paste are intentionally handled differently
- direct PTY text injection is forbidden for AI-pane plain text
- image detection must not be shadowed by premature text fallback
- Windows image paste shortcut is image-specific and must not be reused for plain text

---

## Validation Scenarios

After implementing, verify all of the following on Windows:

### A. AI pane plain-text paste

Paste a large multiline text block into Claude Code.

Expected:

- pasted-block UX is as close as possible to native behavior
- no regression to direct-stream typing semantics
- no more "only first sentence seems visible" behavior

### B. AI pane image paste

Paste an image into Claude Code via right click / paste path.

Expected:

- image paste still behaves like AI-native image input
- image does not degrade into path/text behavior

### C. AI pane mixed clipboard edge cases

Test clipboard content that may include text + rich payload.

Expected:

- image-rich payloads are not prematurely downgraded to plain text

### D. Non-AI pane regression

Expected:

- plain text still works
- image/path fallback still works as before

---

## Acceptance Criteria

- [ ] AI-pane plain text no longer uses direct PTY text injection
- [ ] AI-pane text path is explicitly separated from AI-pane image path
- [ ] AI-pane image paste no longer regresses into path/text behavior
- [ ] clipboard classification for AI panes no longer prioritizes plain text before image fallback in a way that breaks image paste
- [ ] non-AI pane behavior does not regress
- [ ] comments clearly document the text/image distinction

---

## Instruction for Claude / Codex

Implement exactly this follow-up behavior:

- keep AI plain-text paste and AI image paste as separate strategies
- preserve the current fix that avoids direct PTY text injection for AI plain text
- fix clipboard classification / fallback order so AI-pane image paste is not downgraded into plain text/path behavior
- do not regress non-AI pane behavior
- add comments explaining why AI text and AI image paste must be handled differently
