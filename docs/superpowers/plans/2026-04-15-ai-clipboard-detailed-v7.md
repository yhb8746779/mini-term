# Windows AI Clipboard Handling - Detailed Implementation Plan

## Goal

Fix the remaining Windows AI-pane clipboard problems in Mini-Term with a design that is explicit about clipboard **source categories** and does not mix them together.

The target behavior is:

1. **Screenshot-style clipboard images** (WeChat screenshot, QQ screenshot, system screenshot, image editors, etc.)
   - should continue to paste directly in Mini-Term AI panes via right-click
   - should keep the current Mini-Term-enhanced image behavior
   - should not be regressed while fixing Explorer objects

2. **Windows File Explorer copied objects**
   - copied image files
   - copied normal files
   - shell/file-list clipboard objects
   - should no longer be swallowed or misrouted
   - should be handled in a way that gets as close as possible to native PowerShell + Claude Code behavior

3. **Plain text**
   - must remain fast
   - must not reintroduce the previous 1-2 second image-probing delay

---

## Current User-Visible Problems

### Problem A - Screenshot-style clipboard images already have value and must stay working

Mini-Term already provides useful enhanced behavior for some screenshot tools (for example WeChat screenshots):

- right-click paste in AI pane works
- pasted content becomes AI image-style input

This behavior must be preserved and generalized to similar screenshot-image clipboard payloads.

### Problem B - Explorer copied file-system objects still do not paste correctly

When the user copies an image/file directly from Windows File Explorer:

- native PowerShell + Claude Code can often distinguish image/file/object better
- Mini-Term AI pane still fails or no-ops

### Problem C - The current implementation still conflates clipboard object categories

The current code distinguishes:

- `plain-text`
- `raw-image`
- `rich-object`

but the handling is still not sufficient because:

- `raw-image` (true bitmap/image payload) and
- `rich-object` (Explorer file objects / shell objects)

are fundamentally different and must not share the same handling assumptions.

---

## Root Cause Summary

## 1. `rich-object` is not "native handoff"

Current AI-pane `rich-object` handling still only sends a control-character-style fallback rather than achieving true native semantic handoff.

That means Mini-Term is not actually passing Explorer clipboard objects through in a way that reproduces the native PowerShell terminal experience.

## 2. Screenshot image payloads and Explorer file objects are different sources

Mini-Term already has a useful enhanced path for true image clipboard payloads.

But Explorer copied image files are not necessarily clipboard image bitmaps.
They are often file-system clipboard objects.

So this distinction must be made explicit:

- screenshot image payload -> Mini-Term enhanced image handling
- Explorer file object -> separate handling path

## 3. A single `rich-object` bucket is too coarse

`rich-object` may contain:

- copied file list
- copied Explorer image file reference
- shell object / URI-like object
- other non-text clipboard payloads

These should not all be handled the same way.

---

## Required Final Behavior

## Category A - Plain text

If clipboard is clearly plain text:

- use the current fast AI text paste path
- do not add image probing delay back

## Category B - True clipboard image payload

If clipboard contains actual image data:

- keep using the Mini-Term enhanced image path
- preserve existing right-click screenshot paste convenience

## Category C - Explorer file-system clipboard objects

If clipboard contains Windows file-system object references:

- do not treat them as `raw-image`
- do not send them down the screenshot-image path
- do not silently no-op
- instead implement a dedicated Explorer-object path

---

## Key Design Decision

Mini-Term should stop trying to solve this with only one frontend clipboard classifier.

Instead, the implementation should explicitly introduce a **Windows-specific file-list clipboard probe**.

That means:

- frontend Web Clipboard API remains useful for text and image hints
- backend Windows clipboard inspection is required for Explorer file objects

---

## Required Data Model Refactor

Current clipboard kinds are not enough.

Replace or extend the current effective classification into these conceptual kinds:

- `plain-text`
- `raw-image`
- `explorer-files`
- `explorer-image-files`
- `rich-object`
- `empty-or-unknown`

### Meaning

- `plain-text`
  - ordinary text content
- `raw-image`
  - true clipboard image data / bitmap payload
- `explorer-files`
  - one or more file-system paths from Explorer clipboard objects, none classified as image files
- `explorer-image-files`
  - one or more file-system paths from Explorer clipboard objects, and all/primary targets are image files
- `rich-object`
  - other rich clipboard object the app can identify but not yet normalize to file paths
- `empty-or-unknown`
  - no usable signal

---

## Backend Work Required (Windows)

## Add a new clipboard command in Rust

File:
- `src-tauri/src/clipboard.rs`

Add a Windows-only command that reads Explorer file-list clipboard objects using the appropriate Windows clipboard format (for example CF_HDROP-style handling).

### Proposed command shape

```ts
read_clipboard_file_list() -> string[]
```

Return:

- absolute file paths
- empty list if clipboard does not contain Explorer file-list data

### Why this is necessary

Frontend Web Clipboard API cannot reliably distinguish all Explorer clipboard object cases.

Without backend file-list extraction, Mini-Term will keep guessing too broadly with `rich-object`.

---

## Frontend Detection Refactor

File:
- `src/utils/terminalCache.ts`

### New detection flow for AI panes on Windows

Order should become:

1. If Web Clipboard explicitly exposes `image/*`
   - return `raw-image`

2. If Web Clipboard explicitly exposes only `text/plain`
   - return `plain-text` immediately
   - do not image-probe first

3. If Web Clipboard suggests rich object OR is inconclusive
   - on Windows, ask backend `read_clipboard_file_list()`
   - if file list exists:
     - classify file paths by extension / MIME guess
     - if they are image files -> `explorer-image-files`
     - otherwise -> `explorer-files`

4. If still unresolved
   - use existing `readImage()` fallback for true clipboard image payloads
   - use `readText()` fallback for text
   - otherwise `empty-or-unknown`

### Important note

Do not allow backend file-list extraction to slow down obvious plain-text paste.
Plain text must still return fast.

---

## Path Classification Rule

Once backend returns file paths:

- if every path (or the only path) has an image extension
  - classify as `explorer-image-files`
- else
  - classify as `explorer-files`

Use a conservative image-extension list initially, for example:

- `.png`
- `.jpg`
- `.jpeg`
- `.gif`
- `.webp`
- `.bmp`
- `.svg`
- `.ico`
- `.tif`
- `.tiff`

Do not overfit beyond that in v1.

---

## AI-Pane Handling Rules (Final)

### 1. `plain-text`
- keep the current fast AI text paste path

### 2. `raw-image`
- use Mini-Term enhanced screenshot-image paste path
- preserve existing right-click convenience

### 3. `explorer-image-files`
- do **not** route to screenshot-image bitmap path
- do **not** route to generic rich-object no-op
- use a dedicated Explorer image-file path

### 4. `explorer-files`
- do **not** route to screenshot-image path
- use a dedicated Explorer file path

### 5. `rich-object`
- keep as residual fallback only
- do not confuse it with screenshot images or file-list clipboard objects

---

## Dedicated Explorer Path Strategy

This is the most important implementation detail.

### Principle

Mini-Term should not try to force Explorer clipboard objects into the screenshot-image path.

Instead, once file paths are extracted from the clipboard backend, Mini-Term can deterministically pass those paths to the AI CLI.

### Recommended v1 implementation

For `explorer-files` and `explorer-image-files`:

- convert file paths into text form
- pass them through the AI text paste path
- preserve quoting for spaces

Example formatting:

```text
"C:\\path\\to\\file1.png"
"C:\\path\\to\\file2.txt"
```

or a space-separated quoted form if that better matches the AI CLI input expectations.

### Why this is acceptable

Because once Explorer objects are normalized to actual file paths, the behavior becomes deterministic and visible instead of being swallowed.

Even if it is not 100% identical to native PowerShell clipboard-object semantics, it is significantly better than:

- no-op
- wrong image routing
- random `Ctrl+V` fallback

### Important distinction

This Explorer-file path normalization is **not** the same as screenshot-image handling.

- screenshot image data -> image-native path
- Explorer file object -> explicit path paste

That distinction is exactly what the user wants.

---

## Why This Matches User Intent

The user wants both of these to remain true at the same time:

1. Screenshot tools that copy image data directly into clipboard should keep Mini-Term's enhanced image paste behavior.
2. Explorer copied files/images should stop failing and should become usable in AI panes.

This design satisfies both.

---

## Files to Modify

### 1. `src-tauri/src/clipboard.rs`
Add:

- Windows clipboard file-list reader command
- export command in Tauri command list if needed

### 2. `src-tauri/src/lib.rs`
Register the new clipboard file-list command.

### 3. `src/utils/terminalCache.ts`
Refactor:

- clipboard classification
- AI-pane routing logic
- add file-path classification helper
- add Explorer-path paste helper

### 4. Optional type helpers
If needed, extend frontend clipboard payload types to represent:

- `explorer-files`
- `explorer-image-files`

---

## Anti-Patterns to Avoid

Do **not** do any of these:

- route `explorer-image-files` into the screenshot-image bitmap shortcut path
- treat all `rich-object` as screenshot images
- send only `Ctrl+V` for Explorer file objects and hope native handling works
- slow down known plain-text paste by always probing image/file-list first
- remove the existing screenshot-image enhancement just to simplify logic

---

## Validation Scenarios

Verify all of the following on Windows:

### A. Screenshot tool -> clipboard image -> AI pane
Examples:
- WeChat screenshot
- QQ screenshot
- system screenshot
- image editor copy

Expected:
- right-click paste still works directly
- still becomes AI image-style input behavior

### B. Explorer -> copy image file -> AI pane
Expected:
- no longer swallowed
- no longer misrouted into screenshot image path
- resolved as usable file path input in AI pane

### C. Explorer -> copy normal file -> AI pane
Expected:
- no longer swallowed
- becomes usable file path input in AI pane

### D. Plain text -> AI pane
Expected:
- remains fast
- no 1-2 second delay reintroduced

### E. Non-AI pane regression
Expected:
- non-AI text/image behavior remains intact

---

## Acceptance Criteria

- [ ] screenshot-style clipboard images still use Mini-Term enhanced image paste path
- [ ] Explorer file-system clipboard objects are no longer silently lost
- [ ] Explorer image files are no longer treated as screenshot bitmaps
- [ ] Explorer file/image objects are converted into usable path-based input for AI panes
- [ ] plain-text AI paste remains fast
- [ ] comments clearly explain screenshot-image vs Explorer-object distinction

---

## Instruction for Claude / Codex

Implement exactly this behavior:

- preserve Mini-Term's enhanced AI screenshot-image paste path for true clipboard image data
- add a Windows backend clipboard file-list reader for Explorer copied file-system objects
- classify Explorer file-list clipboard content separately from raw clipboard image payloads
- for Explorer file objects, normalize to usable file paths and pass those paths into the AI pane instead of routing them to the screenshot-image path
- keep current fast plain-text AI paste behavior intact
- add comments documenting the difference between screenshot image data and Explorer file objects
