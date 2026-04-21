# AI Clipboard Handling v7 - Preserve Screenshot Image Paste, Let Explorer Objects Use Native Handling

## Goal

Implement the correct final clipboard behavior for AI panes in Mini-Term on Windows.

There are **two different clipboard source categories**, and they must be handled differently:

1. **screenshot-style image clipboard payloads**
   - image data copied directly into the system clipboard by screenshot tools
   - these may not correspond to a real file on disk
   - Mini-Term already provides useful enhanced behavior for these in AI panes
   - this behavior must be preserved

2. **Windows Explorer copied file-system objects**
   - image files
   - normal files
   - shell clipboard objects / file references / file-list style payloads
   - native PowerShell + Claude Code already handles these better
   - Mini-Term should avoid breaking this and should prefer native handling

---

## Product Requirement

### A. Screenshot-style clipboard images

For clipboard content that represents actual image bitmap/image data copied by screenshot tools:

- right-click paste inside AI panes should continue to work directly in Mini-Term
- this should produce the AI image-style input behavior (the `[] image#...`-style UX the user already sees working in some cases)
- this must remain true not just for WeChat screenshots, but for similar screenshot tools as well

### B. Explorer copied file-system objects

For clipboard content copied directly from Windows File Explorer:

- Mini-Term should avoid trying to reinterpret these as screenshot images
- Mini-Term should let the terminal / Claude Code native handling take over as much as possible
- native PowerShell currently distinguishes these better:
  - image file objects
  - normal files
  - other clipboard objects
- Mini-Term should stop blocking or misrouting that native behavior

---

## Key Design Principle

Do **not** treat all non-text clipboard payloads as the same thing.

In particular:

- `raw-image` (real clipboard image data) is **not the same thing as**
- `rich-object` (Explorer file references / shell clipboard objects / file list / unknown rich payloads)

These two categories must use different handling strategies.

---

## Correct Final Strategy

## 1. Plain text in AI panes

Keep the current text-paste improvement path if it already removed the visible delay.

The main focus of this document is **not** text.

---

## 2. Screenshot-style clipboard images in AI panes

If clipboard content is a true image payload:

- keep using Mini-Term's enhanced image handling path
- preserve the current right-click convenience
- do not push these into generic native Explorer/file-object handling

This is the behavior the user explicitly wants to keep.

---

## 3. Explorer copied file-system objects in AI panes

If clipboard content comes from File Explorer and is exposed as a rich clipboard object:

- do **not** route it into the screenshot-image path
- do **not** route it into AI image-specific shortcut handling unless it is positively identified as real image bitmap clipboard data
- prefer the path that lets the native CLI / terminal handling decide what it is

The desired result is:

- Explorer image/file copy should behave closer to native PowerShell + Claude Code
- Mini-Term should stop swallowing or misclassifying those objects

---

## Required Refactor

## A. Split clipboard source handling into 3 distinct classes

In AI panes, clipboard classification must explicitly distinguish:

1. `plain-text`
2. `raw-image`
3. `rich-object`

with `empty-or-unknown` as fallback only.

But more importantly, handling must reflect **source intent**:

- screenshot-like real image data -> Mini-Term enhanced image path
- Explorer file-system object -> native handoff path

---

## B. Preserve Mini-Term image enhancement only for real image payloads

### Required rule

Only use the current Mini-Term image-special path when clipboard is positively identified as actual image content.

Positive identification means:

- `navigator.clipboard.read()` includes `image/*`
- or `readImage()` succeeds and yields actual image data

If that is true:

- keep current image-special handling
- preserve the user's existing right-click screenshot paste experience

---

## C. Stop treating `rich-object` as image

### Current incorrect direction

Some previous iterations treated `rich-object` as if it could be sent down the same path as image-native paste.

That is wrong.

Why:

- Explorer copied file objects are not image bitmaps
- image-native shortcuts are image-specific
- routing Explorer objects into the image path causes failure or no-op

### Required rule

If clipboard is `rich-object`:

- do **not** send it into the image path
- do **not** try to fake it as screenshot image content
- instead, route it to the native-handoff path

---

## D. Native-handoff path for Explorer clipboard objects

This is the key requirement for fixing Explorer copy/paste.

### Required idea

For Explorer-style rich clipboard objects:

- let the terminal / CLI native paste handling take over as much as possible
- Mini-Term should avoid converting them into text/image prematurely

### Important note

This is different from screenshot images.

The logic should be:

- if real image bitmap -> Mini-Term image enhancement path
- if rich Explorer object -> native handoff path

---

## Expected AI-Pane Decision Rules

### Case 1: `plain-text`
- use the AI text path currently chosen by the latest working text implementation

### Case 2: `raw-image`
- use Mini-Term enhanced image path
- preserve current right-click screenshot image UX

### Case 3: `rich-object`
- use native-handoff path
- do not route to image shortcut blindly
- do not force text conversion

### Case 4: `empty-or-unknown`
- conservative fallback
- but do not let this override the two rules above

---

## What Native Handoff Means Here

For this task, "native handoff" means:

- do not reinterpret Explorer clipboard objects as Mini-Term-managed image payloads
- trigger the closest available terminal-native paste behavior instead of trying to synthesize image/text content inside Mini-Term

The exact low-level implementation can vary, but the intent must be preserved:

- Mini-Term enhancement for screenshot image payloads
- native handling for Explorer file-system objects

---

## Files to Modify

### 1. `src/utils/terminalCache.ts`

Primary target.

Needs careful restructuring of:

- clipboard classification
- AI-pane routing logic
- comments documenting why screenshot images and Explorer objects differ

### 2. Any helper functions involved in AI paste routing

If text / image / native-handoff helpers are currently too entangled, split them explicitly.

Suggested conceptual helpers:

- text paste helper
- enhanced screenshot-image paste helper
- Explorer/native-handoff paste helper

The exact names are up to implementation, but the paths must be separate.

---

## Comments That Must Be Added

Please add comments explaining:

- screenshot clipboard image data is different from Explorer clipboard objects
- Mini-Term should preserve screenshot-image enhancement
- Explorer file-system clipboard objects should not be forced into image handling
- the user explicitly wants both behaviors at the same time

---

## Validation Scenarios

After implementation, verify all of these on Windows:

### A. Screenshot tool -> clipboard image -> AI pane

Examples:

- WeChat screenshot
- other screenshot tools that place image data directly into clipboard

Expected:

- right-click paste still works
- AI pane still turns this into image-style input behavior
- this existing convenience is preserved

### B. Explorer -> copy image file -> AI pane

Expected:

- Mini-Term does not swallow or misroute it into screenshot-image handling
- behavior is closer to native PowerShell + Claude Code
- native distinction between image/file/object is preserved as much as possible

### C. Explorer -> copy normal file -> AI pane

Expected:

- object is not lost
- Mini-Term does not incorrectly try to treat it as image bitmap data
- native handling path is given a chance to work

### D. Plain text -> AI pane

Expected:

- current text paste speed remains good
- no regression from this refactor

### E. Non-AI pane regression

Expected:

- non-AI text/image behavior remains as before

---

## Acceptance Criteria

- [ ] screenshot-style clipboard images still paste directly via Mini-Term right-click in AI panes
- [ ] this behavior works not only for WeChat screenshots, but for similar clipboard-image screenshot tools
- [ ] Explorer copied file-system objects are no longer forced into the screenshot-image path
- [ ] `rich-object` is no longer treated as equivalent to `raw-image`
- [ ] Explorer copied image/file objects have a native-handoff path instead of being swallowed
- [ ] plain-text AI paste does not regress in speed
- [ ] comments clearly explain the screenshot-image vs Explorer-object distinction

---

## Instruction for Claude / Codex

Implement exactly this behavior:

- preserve Mini-Term's enhanced right-click paste path for true clipboard image data from screenshot tools
- do not break the existing screenshot-image convenience already working in AI panes
- separate Explorer file-system clipboard objects from true clipboard image data
- stop treating `rich-object` as if it were equivalent to `raw-image`
- route Explorer file-system clipboard objects to a native-handoff path instead of the Mini-Term screenshot-image path
- keep current fast plain-text AI paste behavior intact
- document the distinction clearly in comments
