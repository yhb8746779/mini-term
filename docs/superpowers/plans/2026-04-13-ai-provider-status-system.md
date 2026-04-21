# AI Provider Status System Plan

## Goal

Implement a provider-aware terminal status system for Mini-Term so the UI can clearly distinguish:

- which model is active (`claude`, `codex`, `gemini`)
- whether that model is currently:
  - outputting
  - waiting for user action
  - finished and waiting for the next instruction
  - in error

The design must remain readable on:

- remote desktop sessions
- small laptop screens
- low-quality compressed video streams

Therefore, the implementation must avoid relying on thin outlines, subtle shadows, or outer rings.

---

## Visual Design

### Provider base colors

- Claude
  - complete: `#d98b3a`
  - generating: `#f2a65a`
- Codex
  - complete: `#2f89c5`
  - generating: `#4db6ff`
- Gemini
  - complete: `#2ea56b`
  - generating: `#45c486`
- Shared warning yellow: `#f5c518`
- Shared error red: `#d4605a`
- Shared idle gray: existing muted text color

### Animation semantics

- `idle`
  - gray, solid
- `ai-complete`
  - provider base color, solid
- `ai-generating`
  - provider base color, slow blink
- `ai-awaiting-input`
  - alternate between provider base color and warning yellow, fast blink
- `error`
  - red, solid (preferred for v1)

### Timing

- generating slow blink: ~1.4s cycle
- awaiting-input fast alternate blink: ~0.5s cycle

Do not use border-ring-based semantics. The provider identity must remain visible in the fill color itself.

---

## Runtime State Machine

### State enum

Use the following status values for pane runtime state:

- `idle`
- `ai-complete`
- `ai-generating`
- `ai-awaiting-input`
- `error`

### Provider enum

Use:

- `claude`
- `codex`
- `gemini`

### State meaning

- `idle`
  - normal terminal, not in an AI session
- `ai-generating`
  - recent output detected; AI is actively printing text
- `ai-awaiting-input`
  - explicit prompt indicates the user must approve, choose, confirm, or continue
- `ai-complete`
  - AI session remains active, but the current round appears finished and is waiting for the next user instruction
- `error`
  - terminal output or process state indicates a clear failure

### Detection priority

Per pane, resolve status with this priority:

1. `error`
2. `ai-awaiting-input`
3. `ai-generating`
4. `ai-complete`
5. `idle`

This ensures that yellow only means "you need to act now" and is never used as a fallback for generic silence.

### Suggested transitions

- `idle -> ai-generating`
- `ai-generating -> ai-awaiting-input`
- `ai-generating -> ai-complete`
- `ai-generating -> error`
- `ai-awaiting-input -> ai-generating`
- `ai-awaiting-input -> ai-complete`
- `ai-awaiting-input -> error`
- `ai-awaiting-input -> idle`
- `ai-complete -> ai-generating`
- `ai-complete -> ai-awaiting-input`
- `ai-complete -> idle`

---

## Awaiting-Input Detection

### Principle

Never classify a pane as awaiting input solely because there has been no recent output.

Yellow must only mean explicit user intervention is likely required.

### Strong interaction phrases (high priority)

If recent output matches any of the following, classify as `ai-awaiting-input`:

- `allow`
- `approve`
- `authorization`
- `authorize`
- `grant access`
- `requires approval`
- `requesting approval`
- `do you want to allow`
- `continue?`
- `confirm`
- `are you sure`
- `press enter`
- `press any key`
- `hit enter`
- `choose an option`
- `select an option`
- `pick one`
- `use arrow keys`
- `space to preview`
- `esc to cancel`
- `ctrl+a to`
- `ctrl+b to`
- `y/n`
- `[y/n]`
- `(y/n)`
- `yes/no`

### Medium-confidence phrases

These should increase confidence, but not force yellow alone unless paired with stronger context:

- `input`
- `enter value`
- `selection`
- `choose`
- `select`
- `waiting for input`
- `awaiting input`

### Exclusions

Do not trigger awaiting-input on purely technical text such as:

- `input tokens`
- `output tokens`
- `select * from`
- `approval policy`
- `permissionMode`
- `errorCode`

### Matching scope

Only inspect a recent output window, e.g. the last 10-30 lines or a small rolling text buffer. Do not scan entire session history.

---

## Error Detection

Classify as `error` when recent output or process state strongly indicates failure, for example:

- `fatal`
- `panic`
- `traceback`
- `exception`
- `connection closed`
- `mcp startup failed`
- `permission denied`
- `no such file`
- `command not found`
- `failed to start`
- abnormal PTY exit / command failure where appropriate

If both an error phrase and a strong interaction prompt appear, prefer `ai-awaiting-input` when the practical next step is user intervention rather than passive failure display.

---

## Event Contract

### Recommended payload

Extend the PTY status change event to carry both provider and status.

```ts
export type AiProvider = 'claude' | 'codex' | 'gemini';

export type PaneStatus =
  | 'idle'
  | 'ai-complete'
  | 'ai-generating'
  | 'ai-awaiting-input'
  | 'error';

export interface PtyStatusChangePayload {
  ptyId: number;
  status: PaneStatus;
  provider?: AiProvider;
}
```

Rules:

- `provider` is optional for `idle`
- `provider` must be present for all AI states

---

## UI Rendering Rules

### Project list level

Location: left `PROJECTS` list.

Rules:

- show up to 3 provider dots per project
- fixed order:
  1. Claude
  2. Codex
  3. Gemini
- one dot per provider
- each provider dot shows the highest-priority state currently active for that provider inside the project

Example:

- Claude awaiting input
- Codex generating
- Gemini complete

Then the project row should show 3 dots:

- Claude yellow/orange alternating fast blink
- Codex blue slow blink
- Gemini green solid

### Terminal tab level

Location: top terminal tab row.

Rules:

- each terminal tab shows exactly one provider-aware state dot
- no extra provider text needed in v1
- tooltip should show `provider + human-readable state`

Suggested tooltip text:

- `Claude · 输出中`
- `Codex · 等待你操作`
- `Gemini · 已完成，等待下一条指令`
- `错误`

### Session panel level

Do not mix runtime-state semantics into the history/session list in v1.

The session list should remain a history/recovery tool.
The project dots and terminal tab dots are the runtime-status system.

---

## Aggregation Rules

### Provider-local priority

Within a project, each provider aggregates by:

- `error`
- `ai-awaiting-input`
- `ai-generating`
- `ai-complete`
- `idle`

### Project-level aggregation object

Recommended internal shape:

```ts
interface ProviderProjectStatus {
  provider: 'claude' | 'codex' | 'gemini';
  status: 'ai-complete' | 'ai-generating' | 'ai-awaiting-input' | 'error';
  count: number;
}
```

For v1, `count` may be computed but does not need to be visibly rendered in the UI. It can be exposed in tooltip text later.

---

## File-by-File Implementation Plan

### 1. `src/types.ts`

Update types to support:

- provider on pane runtime state
- refined AI status enum

Expected changes:

- add `AiProvider`
- update `PaneStatus`
- extend `PaneState` with `aiProvider?: AiProvider`

### 2. `src-tauri/src/pty.rs`

Responsibilities:

- detect provider on AI-session entry (`claude`, `codex`, `gemini`)
- clear provider on AI-session exit
- maintain rolling recent-output buffers
- maintain timestamps or markers for:
  - recent output
  - recent interaction prompt hit
  - recent error hit

### 3. `src-tauri/src/process_monitor.rs`

Responsibilities:

- replace current two-state AI logic (`ai-generating` / `ai-working`) with the refined state machine
- emit `provider + status` together

### 4. `src/store.ts`

Responsibilities:

- update pane runtime state using `provider + status`
- change project-level aggregation to provider-aware aggregation
- preserve existing notification/DONE-tag behavior unless intentionally redesigned

### 5. `src/components/StatusDot.tsx`

Responsibilities:

- accept provider-aware state input
- map `provider + status` to:
  - color
  - animation
  - tooltip text

### 6. `src/styles.css`

Responsibilities:

- define provider colors
- define generating slow blink animation
- define awaiting-input alternating color animation

Suggested variable groups:

- `--color-claude-complete`
- `--color-claude-generating`
- `--color-codex-complete`
- `--color-codex-generating`
- `--color-gemini-complete`
- `--color-gemini-generating`
- `--color-warn`

### 7. `src/components/ProjectList.tsx`

Responsibilities:

- render up to 3 provider dots per project
- preserve fixed provider order
- use aggregated provider-aware state instead of a single combined AI status

### 8. Terminal-tab component

The current codebase already renders terminal tab status dots somewhere in the terminal header/tab row.
Locate that component and update it so that the tab dot becomes provider-aware with the same visual language as the project list.

---

## Acceptance Checklist

### Data model

- [ ] panes can carry `provider`
- [ ] panes can carry refined AI status

### Detection

- [ ] Claude sessions map to `provider=claude`
- [ ] Codex sessions map to `provider=codex`
- [ ] Gemini sessions map to `provider=gemini`
- [ ] explicit interaction prompts produce `ai-awaiting-input`
- [ ] simple silence does not automatically produce `ai-awaiting-input`
- [ ] recent output produces `ai-generating`
- [ ] finished rounds can settle into `ai-complete`

### Project UI

- [ ] projects can show separate Claude/Codex/Gemini dots
- [ ] two providers awaiting input at the same time remain visually distinguishable
- [ ] provider order is fixed and stable

### Tab UI

- [ ] terminal tab dots reflect current provider + status
- [ ] tooltip text is understandable without reading raw enum names

### Animation

- [ ] generating slow blink is visibly different from awaiting-input fast alternate blink
- [ ] complete is solid and non-flashing
- [ ] animations remain readable over remote desktop compression

### Regression safety

- [ ] existing DONE-tag logic still works
- [ ] existing completion notifications still work
- [ ] project switching does not become more janky
- [ ] session/history panel behavior is not broken

---

## Non-Goals for v1

Do not implement the following in the first pass unless necessary:

- a separate `ai-blocked` state
- counts rendered next to provider dots
- session-panel runtime status overlays
- outline/ring-based status differentiation
- fancy gradient/glow-heavy effects

Focus on robust provider-aware dots first.

---

## Implementation Notes for the Coding Agent

When implementing:

- keep the status logic conservative
- prefer under-reporting yellow over over-reporting yellow
- yellow must mean: user likely needs to do something now
- preserve current responsiveness; do not reintroduce blocking scans or UI jank
- avoid large refactors outside the provider-status feature scope
