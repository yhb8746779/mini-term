# Explorer Image Files Should Not Be Forced Into Path-Only Paste

## Goal

Fix the remaining AI-pane clipboard mismatch for Windows File Explorer copied content.

Current behavior still treats:

- `explorer-image-files`
- `explorer-files`

as the same path-injection behavior.

This is too coarse.

The user expectation is:

- Explorer copied **image files** should behave closer to native PowerShell + Claude Code image handling
- Explorer copied **normal files** may reasonably become path-style input

So these two cases must no longer share the same final handling path.

---

## Current Incorrect Behavior

Current AI-pane routing does this:

- `explorer-image-files` -> `sendAiExplorerFilesPaste()`
- `explorer-files` -> `sendAiExplorerFilesPaste()`

And `sendAiExplorerFilesPaste()` simply formats file paths as text and pastes them as text.

That means:

- Explorer copied image files are always downgraded into file-path text
- they can never become image-style input blocks

This is exactly why the user still sees:

- sometimes pasted image becomes path-style text instead of image block style

---

## Required Fix

### Core rule

Do not treat `explorer-image-files` and `explorer-files` as the same final behavior.

They must be split.

### Final requirement

#### `explorer-files`
- may continue using path-style text injection
- this is acceptable and matches user expectations for normal files

#### `explorer-image-files`
- must no longer be automatically downgraded into path text
- must get their own dedicated handling path

---

## Why the Current Behavior Is Wrong

Explorer image files are still file-system objects, not raw clipboard bitmaps.
That is true.

But the user's expectation is not:

- "treat image files exactly like ordinary files"

The user's expectation is:

- when Explorer copied content represents image files, Mini-Term should not flatten that into plain path-style UX if a more image-like/native path is available

So even though `explorer-image-files != raw-image`, it still deserves a separate strategy.

---

## Required Refactor

## 1. Split the current shared branch

Current branch roughly looks like:

```ts
if (
  clipboard.kind === 'explorer-image-files' ||
  clipboard.kind === 'explorer-files'
) {
  sendAiExplorerFilesPaste(...)
}
```

This must be split into two separate branches.

### Required shape

```ts
if (clipboard.kind === 'explorer-image-files') {
  // dedicated image-file handling
} else if (clipboard.kind === 'explorer-files') {
  // path-style file handling
}
```

---

## 2. Keep `explorer-files` on the path-text strategy

For normal file objects copied from Explorer:

- preserve the current path-text strategy
- quoted absolute paths are acceptable

This part is fine.

---

## 3. Introduce a dedicated `explorer-image-files` strategy

This is the missing piece.

### Design requirement

Do not immediately collapse Explorer image files into plain path text.

Instead, create a dedicated helper for example conceptually like:

```ts
sendAiExplorerImageFilesPaste(...)
```

### What this helper should do

The implementation should prefer the best available image-like/native behavior for image files, rather than blindly pathifying them.

If full native object handoff is not realistically available, then the code should still remain explicitly split so future refinement is possible.

### Minimum acceptable v1 fix

Even if the final behavior must temporarily still use paths in some fallback scenario, the code must no longer hardwire `explorer-image-files` into the same generic function as ordinary files.

That means:

- separate helper
- separate branch
- separate comments
- separate intent

Without that split, future improvement is impossible and the wrong UX is locked in.

---

## 4. Preserve screenshot/raw-image behavior

Do **not** regress the current screenshot/raw clipboard image path.

`raw-image` should continue using the Mini-Term enhanced screenshot-image handling path.

This task is only about the incorrect flattening of `explorer-image-files`.

---

## 5. Documentation requirement

Add comments explaining:

- `raw-image` = screenshot/bitmap clipboard payload
- `explorer-image-files` = file-system image references copied from Explorer
- `explorer-files` = non-image file-system references
- these three categories are different and must not be collapsed into one path

---

## Validation Scenarios

### A. Explorer copy image file -> AI pane

Expected:

- no longer forced through the generic path-text helper for ordinary files
- image files now use a dedicated branch
- behavior should move closer to intended image-style UX

### B. Explorer copy normal file -> AI pane

Expected:

- still becomes path-style input
- no regression

### C. Screenshot clipboard image -> AI pane

Expected:

- existing screenshot image enhancement still works

---

## Acceptance Criteria

- [ ] `explorer-image-files` and `explorer-files` no longer share the same final helper
- [ ] normal files still use path-style handling
- [ ] Explorer image files have a dedicated strategy branch
- [ ] screenshot/raw-image behavior is unchanged
- [ ] comments clearly document the distinction

---

## Instruction for Claude / Codex

Implement exactly this change:

- split `explorer-image-files` and `explorer-files` into separate AI-pane branches
- keep `explorer-files` on the current path-text behavior
- introduce a dedicated helper/path for `explorer-image-files` instead of routing them through the generic Explorer-files text path
- do not change `raw-image` screenshot behavior
- document clearly why screenshot bitmap images, Explorer image files, and ordinary files are three different clipboard categories
