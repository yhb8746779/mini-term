# AI Paste Fix v6 - Low-Latency Text Path and Explorer Clipboard Objects

## Goal

Fix two remaining Windows AI-pane paste problems in Mini-Term:

1. **Plain-text paste in AI panes is too slow** (visible 1-2 second lag)
2. **Windows File Explorer copied images/files still cannot be pasted into AI panes**

This document is intentionally detailed because previous implementations mixed text, image, and rich clipboard object handling and introduced repeated regressions.

---

## Problem Summary

### Problem A - plain-text paste is delayed

Current AI-pane clipboard detection uses `preferImage=true`, which means:

- `navigator.clipboard.read()` may already provide `text/plain`
- but the code still defers returning it
- then performs `readImage()` first
- only after image detection fails does it return the deferred text

This adds unnecessary delay to ordinary text paste.

### Problem B - Explorer copied images/files still do not paste into AI panes

Current logic treats `rich-object` as if it were equivalent to `raw-image` and routes it to the AI image shortcut path (`Alt+V` on Windows).

This is incorrect.

Reason:

- `raw-image` means clipboard contains actual image bitmap/image payload
- `rich-object` may instead mean:
  - Explorer file references
  - shell clipboard objects
  - URI/file-list style payloads
  - non-image rich clipboard data

Those must **not** be blindly handled as native image paste.

---

## Main Design Rule

AI-pane paste must be split into **three different classes**:

1. plain text
2. true image clipboard payload
3. Windows Explorer-style file/rich clipboard objects

These are **not interchangeable**.

---

## Required Behavior

## A. Plain-text in AI panes

### Required result

- should be fast
- should not wait on image detection if text is already clearly known
- should preserve AI pasted-block semantics as much as possible

### Required implementation principle

If `navigator.clipboard.read()` already gives valid `text/plain` and there is **no explicit image signal** and **no rich-object signal**, return plain text immediately.

Do **not** continue into `readImage()` first in that case.

### Why

For ordinary copied text, image detection is irrelevant and creates the visible lag you are seeing.

---

## B. True image clipboard payload in AI panes

### Required result

- keep current image-native shortcut behavior
- Windows image-native shortcut remains valid for actual clipboard image content

### Required implementation principle

Only use the AI image-native shortcut path when clipboard classification is truly `raw-image`.

That means:

- actual `image/*` from Web Clipboard API
- or successful image decoding from `readImage()`

### Important rule

Do **not** route generic `rich-object` into image shortcut handling.

---

## C. Explorer copied files / clipboard objects in AI panes

### Required result

When clipboard content comes from Windows File Explorer and is exposed as rich clipboard objects:

- do not silently no-op
- do not incorrectly treat it as text
- do not incorrectly treat it as image bitmap paste

### Expected handling strategy

If clipboard is classified as `rich-object` and is **not** a true image:

- keep it separate from `raw-image`
- use a dedicated fallback path for AI panes

If Mini-Term cannot truly reconstruct Explorer file-object paste into the AI CLI in a reliable way, then the behavior should at least be:

- explicit and deterministic
- not misrouted into image logic
- not delayed by unrelated image probing

At minimum, the code must stop assuming:

> rich-object == image

because that is what is currently wrong.

---

## Required Refactor

## 1. Refactor clipboard classification logic

File:
- `src/utils/terminalCache.ts`

### Current issue

`detectClipboardPayload(preferImage)` is overloaded and currently couples too many decisions into one flow.

### Required change

Refactor classification so AI-pane decision-making distinguishes:

- explicit image
- explicit plain text
- rich-object
- unknown

### Strong requirement

For AI panes, use this decision order:

#### Case 1: Web Clipboard explicitly says image
Return:
- `raw-image`

#### Case 2: Web Clipboard explicitly says rich object (non text/html/plain, non image)
Return:
- `rich-object`

#### Case 3: Web Clipboard explicitly gives only text/plain
Return immediately:
- `plain-text`

Do not run `readImage()` first in this case.

#### Case 4: Clipboard API unavailable / inconclusive
Then and only then use fallback probing:

- try `readImage()`
- then try `readText()`

This is the key fix for text latency.

---

## 2. Stop treating `rich-object` as image in AI panes

### Current incorrect behavior

Current AI-pane branch effectively does:

- `raw-image` -> image shortcut
- `rich-object` -> image shortcut too

This is wrong.

### Required behavior

AI-pane branch must distinguish:

- `raw-image` -> image shortcut path
- `plain-text` -> AI text paste path
- `rich-object` -> dedicated rich-object fallback path
- `empty-or-unknown` -> conservative fallback

### Important note

If you cannot fully support Explorer file-object native AI paste today, that is acceptable.
But you must not keep routing it into image-specific paste behavior.

---

## 3. Text path must not be blocked by image probing

### Required behavior

When AI pane clipboard is plain text:

- text path should execute immediately
- no image probing first

### Acceptance signal

Plain text paste should no longer feel like it stalls for 1-2 seconds before content appears.

---

## Suggested Data Model

Keep these payload kinds:

```ts
type ClipboardPayloadKind =
  | 'plain-text'
  | 'raw-image'
  | 'rich-object'
  | 'empty-or-unknown';
```

But classification logic must stop conflating `rich-object` and `raw-image`.

---

## AI Pane Handling Rules (Final)

### If `clipboard.kind === 'plain-text'`
- use AI text paste path immediately
- do not image-probe first once text is already confidently known

### If `clipboard.kind === 'raw-image'`
- use AI image-native shortcut path

### If `clipboard.kind === 'rich-object'`
- do not use image shortcut blindly
- route to a dedicated rich-object fallback path
- if no reliable implementation exists yet, leave a clearly documented fallback instead of wrong routing

### If `clipboard.kind === 'empty-or-unknown'`
- use conservative fallback
- but do not make this path responsible for ordinary text latency

---

## Anti-Patterns to Avoid

Do **not** do any of the following:

- delay known plain-text AI paste by probing image first
- treat every `rich-object` as image
- use `Alt+V` for Explorer file objects unless you have positively identified real image clipboard content
- use one unified AI fallback path for text/image/rich-object
- silently no-op without comments or intentional fallback behavior

---

## Comments That Must Be Added

Please add comments explaining:

- plain text must not wait on image probing once text is already confidently known
- `raw-image` and `rich-object` are different concepts
- Explorer clipboard objects are not equivalent to image bitmaps
- image-native shortcut must only be used for actual image clipboard payloads

---

## Validation Scenarios

After implementation, verify these on Windows:

### A. AI pane plain text
- copy large plain text from a normal editor
- paste into Claude Code in Mini-Term
- expected:
  - paste reacts quickly
  - no obvious 1-2 second delay
  - AI pasted-block UX remains as good as current implementation allows

### B. AI pane clipboard image
- copy an actual bitmap/image from a screenshot tool or image editor
- paste into Claude Code in Mini-Term
- expected:
  - image-native paste path still works

### C. AI pane Explorer-copied image/file
- copy an image file or file object directly from Windows Explorer
- paste into Claude Code in Mini-Term
- expected:
  - not silently ignored
  - not incorrectly treated as plain text
  - not incorrectly routed into image-only path unless it is truly image bitmap clipboard content

### D. Non-AI pane regression
- normal plain text still pastes normally
- non-AI image/path handling still works

---

## Acceptance Criteria

- [ ] plain-text AI paste no longer waits on image probing when text is already confidently known
- [ ] visible text-paste lag is removed or significantly reduced
- [ ] `raw-image` is handled separately from `rich-object`
- [ ] `rich-object` is no longer blindly sent to the image shortcut path
- [ ] Explorer-copied image/file objects are no longer silently lost due to wrong routing
- [ ] comments clearly explain the distinction between plain text, raw image, and rich-object handling

---

## Instruction for Claude / Codex

Implement exactly this behavior:

- refactor AI-pane clipboard classification so that explicit plain text returns immediately and does not wait for image probing
- keep true image clipboard payloads on the image-native path
- stop treating `rich-object` as equivalent to `raw-image`
- add a separate AI rich-object fallback path instead of routing rich objects into image paste blindly
- keep non-AI paste behavior unchanged unless necessary
- document the distinctions clearly in comments
