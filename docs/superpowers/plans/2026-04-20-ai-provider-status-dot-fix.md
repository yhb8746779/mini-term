# AI Provider Status Dot Fix Plan

## Goal

Fix the provider/status-dot bugs in Mini-Term so that terminal tabs and project-level dots behave reliably when users launch AI sessions via:

- direct typing
- up-arrow history recall
- right-arrow / autosuggestion accept
- PSReadLine / zsh autosuggestion / themed prompts

The user-facing failures that must be fixed are:

- tab/project dot sometimes stays gray / appears missing
- Claude pane sometimes shows Codex color
- Codex pane sometimes shows Claude color
- provider color appears only after exiting/re-entering multiple times
- status gets stuck in awaiting-input because of overly broad prompt matching

This document is intentionally precise because the current bugs come from multiple interacting issues, not one single rendering bug.

---

## Problem Summary

There are 4 root causes that interact:

### 1. AI-session detection and provider detection are not symmetric

The code can detect "this is an AI session" more easily than it can detect "which provider it is".

So a session may enter AI state successfully while provider remains `None`.

### 2. A session may be marked as AI before provider is available

`try_enter_ai_from_recent_output()` can mark `is_ai_session = true` before provider is reliably stored.
Once that happens, later Enter handling goes down the `in_ai` branch and may skip the normal provider-detection path.

This creates long-lived missing-provider sessions.

### 3. Frontend keeps stale provider when provider is missing

When a new AI status arrives with `provider = undefined`, frontend state currently keeps the previous provider unless status becomes `idle`.

That causes provider color bleed:

- Claude pane can remain blue (old Codex provider)
- Codex pane can remain orange (old Claude provider)

### 4. Awaiting-input prompt matching is too broad

Some generic UI phrases (especially arrow-key guidance) can keep panes stuck in `ai-awaiting-input`, making colors blink incorrectly long after the actual interaction state changed.

---

## Primary Fix Objectives

1. Provider detection must be as reliable as AI-session detection
2. Provider must be recoverable even if initial detection missed it
3. Frontend must never keep stale provider color when provider is absent
4. Awaiting-input detection must be narrowed to explicit user-action prompts

---

## Detailed Fix Plan

## Part A - Unify AI detection and provider detection

### Current issue

Right now the code path that answers:

- "is this an AI command?"

is more permissive than the code path that answers:

- "which provider is it?"

That asymmetry is the foundation of the bug.

### Required change

Make provider extraction use the same conceptual detection space as AI detection.

If `output_contains_ai_command(...)` says "yes, this is an AI command echo", provider extraction must not use a weaker parser that fails on the same line.

### Implementation direction

Refactor the detection so that a single parsing routine returns something like:

```rust
struct DetectedAiCommand {
    provider: &'static str,
}
```

Instead of:

- one function returning `bool`
- another function separately trying to infer provider later

### Recommended design

Replace the split between:

- `output_contains_ai_command(...)`
- `detect_provider_from_output(...)`

with a unified routine that returns:

- `Some(provider)` if a provider-bearing AI command is found
- `None` otherwise

That routine must be used consistently in:

- direct command parsing
- history/autosuggestion/output-based detection paths

---

## Part B - Eliminate the session-before-provider race problem

### Current issue

`try_enter_ai_from_recent_output()` can do:

- insert into `ai_sessions`
- and only later try to write `ai_providers`

This creates a window where monitor sees:

- AI session = true
- provider = None

and emits providerless AI state.

### Required change

Make AI-session state and provider state update atomically from the perspective of runtime logic.

### Acceptable approaches

#### Option 1 (preferred)
Use one lock scope to write both session and provider together.

Meaning:

- detect provider first
- then insert session + provider in the same logical critical section

#### Option 2
If provider cannot be resolved, do not mark the pane as AI yet in that path.

Only transition to AI state when provider is known.

This is stricter, but may delay AI recognition slightly. Acceptable if the UX remains stable.

### Avoid

Do not keep the current sequence:

- set session first
- maybe provider later

That is exactly what causes gray/no-color panes.

---

## Part C - Add provider backfill after AI session has already started

### Current issue

Once `is_ai_session == true`, some code paths stop trying to detect provider.

So if provider was missed at entry time, the pane may remain providerless until full exit/re-entry.

### Required change

Add a provider backfill mechanism.

### Rule

If a pane is already in AI session and:

- provider is still missing
- new PTY output arrives
- and that output clearly identifies provider

then provider should be filled in and emitted to frontend.

### Suggested implementation points

Possible places:

- PTY output processing path
- monitor loop before emit
- a dedicated provider-reconciliation helper

### Important requirement

Do not require the user to leave and re-enter the conversation just to recover provider color.

---

## Part D - Frontend must clear stale provider when provider is absent

### Current issue

Frontend update logic currently behaves like:

- if provider exists -> overwrite `aiProvider`
- if status is idle -> clear `aiProvider`
- otherwise -> keep previous provider

This is what causes cross-session color bleed.

### Required change

When a pane receives an AI status update with no provider, frontend must **not** keep the old provider color indefinitely.

### Recommended behavior

If incoming state is AI but provider is missing:

- clear `aiProvider`
- show a neutral state instead of stale provider color

This is much safer than showing the wrong model color.

### Why

Gray/unknown is less misleading than blue-for-Claude or orange-for-Codex.

### Result

- provider missing -> neutral/unknown appearance
- provider found later -> update to correct provider color

---

## Part E - Narrow awaiting-input detection

### Current issue

Current awaiting-input phrase matching is too broad.

This can keep panes stuck in `ai-awaiting-input` long after the user already interacted.

### Required change

Restrict awaiting-input detection to genuinely explicit interaction prompts.

### Examples of phrases that should remain

- `do you want to allow`
- `requires approval`
- `requesting approval`
- `press enter`
- `press any key`
- `choose an option`
- `select an option`
- `y/n`
- `yes/no`

### Examples that should be reviewed / likely removed or handled more carefully

- `use arrow keys`
- generic `confirm`
- generic `authorize`
- generic `allow`

If a phrase appears too often in normal descriptive output, it should not independently force `ai-awaiting-input`.

### Additional recommendation

Consider reducing the persistence of the output window used for awaiting-input detection, or tagging whether a prompt has already been consumed by user input.

---

## Part F - Monitoring behavior

### Current issue

Monitor simply emits whatever provider/status snapshot it sees at that instant.

If provider is temporarily missing, frontend gets unstable data.

### Required change

Before emitting provider-aware AI statuses, ensure that either:

- provider is known
- or the status is downgraded to a neutral state that does not preserve stale provider identity

Do not emit providerless AI state if frontend will interpret it as "keep old color".

---

## Suggested File-by-File Changes

### 1. `src-tauri/src/pty.rs`

Required work:

- unify AI command detection and provider extraction
- make AI-session + provider writes atomic or logically inseparable
- add provider backfill when `is_ai_session == true` but provider missing
- ensure history/autosuggestion paths can recover provider consistently

### 2. `src-tauri/src/process_monitor.rs`

Required work:

- do not emit misleading providerless AI state as if it were complete information
- optionally reconcile provider before final status emit
- narrow awaiting-input phrase logic

### 3. `src/store.ts`

Required work:

- when AI status arrives with no provider, clear `aiProvider` instead of keeping stale provider
- keep provider updates monotonic and safe

### 4. `src/components/StatusDot.tsx`

Required work:

- handle missing provider in AI states as neutral/unknown instead of relying on old color
- tooltip may optionally clarify `AI · provider unknown` if useful

---

## Acceptance Criteria

### Provider correctness

- [ ] direct typing `claude` sets Claude provider color reliably
- [ ] direct typing `codex` sets Codex provider color reliably
- [ ] up-arrow recalled `claude` sets Claude provider color reliably
- [ ] up-arrow recalled `codex` sets Codex provider color reliably
- [ ] right-arrow / autosuggestion accepted `claude` sets Claude provider color reliably
- [ ] right-arrow / autosuggestion accepted `codex` sets Codex provider color reliably

### No stale color bleed

- [ ] a pane that previously ran Codex cannot keep blue color when a new Claude session starts and provider is temporarily missing
- [ ] a pane that previously ran Claude cannot keep orange color when a new Codex session starts and provider is temporarily missing

### No long-lived gray due to missing provider

- [ ] provider can be recovered without requiring full exit/re-entry
- [ ] tab/project dots do not remain gray forever for active AI sessions

### Awaiting-input reliability

- [ ] panes do not stay stuck in awaiting-input due to generic arrow-key help text
- [ ] awaiting-input only appears for explicit user-action prompts

### Regression safety

- [ ] existing provider-aware status dots still work when provider is known immediately
- [ ] project-level provider aggregation still works
- [ ] DONE/notification logic still behaves as intended

---

## Validation Scenarios

Test all of the following manually:

1. Start Claude by typing command directly
2. Start Codex by typing command directly
3. Recall Claude using up-arrow and press Enter
4. Recall Codex using up-arrow and press Enter
5. Accept Claude autosuggestion using right-arrow and press Enter
6. Accept Codex autosuggestion using right-arrow and press Enter
7. Switch from Codex session to Claude session in the same pane and verify color updates correctly
8. Switch from Claude session to Codex session in the same pane and verify color updates correctly
9. Trigger an explicit approval prompt and verify awaiting-input appears
10. Navigate a UI that prints generic arrow-key help and verify it does not stay stuck incorrectly

---

## Instruction for Claude / Codex

Implement exactly this fix strategy:

- unify AI-session detection and provider extraction so provider is not weaker than session detection
- eliminate the race where session becomes true before provider is stored
- add provider backfill so missing provider can be recovered after session start
- on frontend, never keep stale provider color when provider is absent; clear to neutral instead
- narrow awaiting-input matching so generic arrow-key help text does not keep panes stuck in that state
