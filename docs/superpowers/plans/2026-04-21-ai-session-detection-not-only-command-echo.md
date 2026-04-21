# AI Session Detection Must Not Rely Only on Command Echo

## Goal

Fix the remaining Mini-Term bug where provider/status dots still sometimes stay gray when the user starts Claude/Codex/Gemini through:

- up-arrow history recall
- right-arrow prediction accept
- PSReadLine / shell autosuggestion workflows

Even though the AI CLI has clearly started and its banner is already visible, Mini-Term still sometimes fails to mark the pane as an AI session quickly enough.

This document focuses only on that problem.

---

## User-Visible Problem

Observed behavior:

- user recalls a previous command using up-arrow or right-arrow
- command is visibly accepted in the terminal
- Claude Code / Codex banner already appears
- but the terminal tab/project status dot remains gray or looks like it has no AI status

Important clarification:

- this is no longer primarily a stale-color bug
- the stale-provider issue has already been reduced
- the remaining problem is that AI session detection itself can still be missed in some history/autosuggestion entry paths

---

## Root Cause

## Current architecture still depends too much on command echo text

Mini-Term currently uses text-oriented detection to infer AI session start:

- direct input buffer parsing
- output echo parsing (`output_since_enter` / `recent output`)
- provider extraction from echoed command lines

This works well when the terminal reliably emits a parseable command echo.

But with:

- PSReadLine
- history recall
- right-arrow accept
- autosuggestion
- custom prompts / themed prompts
- repaint/overwrite behavior

that assumption is not reliable enough.

### Result

Sometimes the AI CLI really has started, but Mini-Term has not yet recognized:

- `is_ai_session = true`
- or the provider associated with that session

That is why the dot can remain gray even while the AI banner is already on screen.

---

## Why the Current Fixes Are Not Enough

The recent provider fixes improved:

- provider parsing symmetry
- stale provider clearing
- provider backfill

But they still assume one key thing:

> the system must first successfully infer the AI session start from input/echo text

If AI session start itself is missed, then:

- provider backfill cannot help enough
- frontend has no AI status to render
- the pane remains effectively idle/gray

So the remaining bug is now primarily:

## AI session start detection is still too text-dependent

---

## Required Design Change

Mini-Term needs a **two-layer AI session detection model**:

### Layer 1 — fast path (existing text-based detection)

Use existing command/input/output parsing to detect AI session start as early as possible.

This remains useful because:

- it is fast
- it works for direct command entry
- it can often detect provider before process polling catches up

### Layer 2 — authoritative fallback (process-based confirmation)

If the text-based path misses the AI session start, Mini-Term must still be able to recover based on the actual running process tree / child process state.

This is the missing piece.

---

## Core Principle

Text parsing should be treated as:

- a fast heuristic

Process detection should be treated as:

- the durable fallback truth

Mini-Term should no longer require command echo visibility in order to recognize that an AI session has started.

---

## Proposed Fix Strategy

## Part A - Keep current text-based detection as fast path

Do not delete the current text-based logic.

It should still be used for:

- direct typed commands
- many normal prompt/echo cases
- early provider assignment when available

This gives the UI quick reaction when the signal is available.

---

## Part B - Add process-level AI session reconciliation

### Problem to solve

If a pane is still considered non-AI after command acceptance, but the underlying PTY process tree now clearly contains Claude/Codex/Gemini, Mini-Term must transition that pane into AI state anyway.

### Required behavior

The process monitor should be able to do something conceptually like:

- inspect the PTY's active process tree / command line / known child processes
- determine whether the pane is now running:
  - Claude Code
  - Codex
  - Gemini
- if yes:
  - mark the pane as AI session even if text-based detection missed it
  - fill provider accordingly

### Important note

This process-based detection is not replacing the existing text path.
It is a recovery/fallback mechanism.

---

## Part C - Provider must also be recoverable from process information

### Why

Even if text-based provider parsing misses the provider, the process layer may still know what is actually running.

For example:

- Claude binary/process name visible
- Codex binary/process name visible
- Gemini process name visible

### Required behavior

If process-level fallback detects AI provider, it must be allowed to backfill:

- `is_ai_session = true`
- `provider = claude/codex/gemini`

without needing the user to exit and re-enter the session.

---

## Part D - Frontend should continue treating missing provider as neutral

The recent frontend fix is still correct:

- if AI status arrives but provider is absent, do not keep stale old provider color
- render neutral/unknown rather than wrong color

This should stay.

But once process-level reconciliation succeeds, provider should be backfilled and the correct color should appear automatically.

---

## Where to Implement

## 1. `src-tauri/src/pty.rs`

Keep existing fast-path text detection, but do not rely on it as the only way to enter AI state.

This file remains responsible for:

- direct command detection
- history/autosuggestion text heuristics
- early provider inference when possible

## 2. `src-tauri/src/process_monitor.rs`

This is where the missing fallback should be added.

### Required enhancement

The monitor must do more than just:

- read `is_ai_session`
- read `provider`
- map recent output to status

It also needs a reconciliation step like:

- if `is_ai_session` is currently false or provider missing
- inspect the actual PTY process / child process information
- infer provider from runtime process state
- update AI session state accordingly

### In practice

If the codebase already has process-monitoring capabilities that know which child processes are alive, reuse them.
If not, add a focused provider inference helper.

---

## Suggested Runtime State Logic

### Current rough flow

- text says AI command -> set AI session -> emit provider-aware state

### Required improved flow

1. Try text-based detection first
2. If text-based detection fails or provider missing:
   - ask process-level detector
3. If process-level detector sees Claude/Codex/Gemini:
   - mark AI session true
   - backfill provider
4. Then compute status dot state from:
   - provider
   - recent output
   - awaiting-input phrases

This ensures the session can recover automatically even when command echo was missed.

---

## What Counts as Success

After this fix, these scenarios should all work reliably:

### Scenario 1 - Direct typed command
- type `claude`
- provider dot becomes Claude quickly

### Scenario 2 - Up-arrow recalled command
- recall `claude --model sonnet`
- press Enter
- even if command echo parsing misses it initially, process fallback should recover the AI session/provider shortly after

### Scenario 3 - Right-arrow accepted autosuggestion
- accept `codex ...` via shell suggestion
- provider dot should still become Codex without needing exit/re-enter

### Scenario 4 - Same pane provider switch
- pane previously ran Codex
- user later runs Claude through history recall
- provider must eventually switch to Claude correctly
- no permanent blue/orange mismatch

---

## Anti-Patterns to Avoid

Do **not** do any of the following:

- continue relying only on command echo parsing
- require user exit/re-enter to recover provider color
- keep stale provider color when provider is missing
- add more and more prompt regexes while ignoring process-level truth

The key point is: this is no longer just a text-parsing problem.

---

## Validation Scenarios

After implementing, verify at minimum:

### A. Direct command start
- type Claude directly
- type Codex directly
- type Gemini directly

Expected:
- immediate or near-immediate correct provider color

### B. Up-arrow recalled command
- recall Claude history entry
- recall Codex history entry

Expected:
- no long-lived gray dot
- no wrong provider color retained

### C. Right-arrow accepted prediction
- accept Claude suggestion
- accept Codex suggestion

Expected:
- process fallback recovers provider even if text path misses it

### D. Same-pane provider swap
- run Codex
- exit
- run Claude via history recall

Expected:
- provider color changes correctly
- no stale color bleed

---

## Acceptance Criteria

- [ ] AI session start no longer depends solely on command echo being parsed
- [ ] provider can be recovered via process-level fallback when text detection misses
- [ ] up-arrow history recall no longer leaves the tab gray indefinitely
- [ ] right-arrow autosuggestion accept no longer leaves the tab gray indefinitely
- [ ] provider eventually becomes correct without forcing user to exit/re-enter the session
- [ ] stale provider color is not preserved when provider is actually unknown

---

## Instruction for Claude / Codex

Implement exactly this fix strategy:

- keep the current text-based AI session/provider detection as a fast path
- add a process-level fallback/reconciliation path so AI session start and provider can still be recovered even when command echo parsing misses history/autosuggestion launches
- ensure provider can be backfilled after session start without requiring exit/re-entry
- preserve the frontend rule that missing provider should render neutral rather than reusing stale provider color
