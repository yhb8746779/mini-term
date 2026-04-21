# Claude Third-Window Provider Debug Plan

## Goal

Diagnose the specific runtime issue:

- Window 1 starts Claude via right-arrow/history accept -> orange is correct
- Window 2 starts Codex via right-arrow/history accept -> blue corrects successfully
- Window 3 starts Claude via the same mechanism -> color does not correct back to orange

This document is **diagnosis-only**. Do not implement speculative fixes before collecting evidence.

---

## Working Hypothesis

The most likely state for the third Claude window is:

- `is_ai_session == true`
- `provider == None`

If that is true, then:

- the pane itself may remain gray or neutral
- project-level aggregation ignores that pane because it only aggregates panes with `aiProvider`
- the only visible provider in the project remains the blue Codex pane
- user perceives this as "Codex corrected to blue, but Claude cannot correct back to orange"

This is more likely than stale-color retention, because the frontend now clears provider when provider is absent.

---

## What Must Be Verified

## 1. Verify the third Claude window's internal runtime state

At the moment the third Claude window is visibly running and its banner is already on screen, inspect whether Mini-Term believes:

- AI session is true or false
- provider is set or missing

### Exact questions

- For the third Claude pane, is `is_ai_session` true?
- For the same pane, what is `ai_providers[pty_id]`?
- Does frontend receive a `pty-status-change` with:
  - `status = ai-generating / ai-thinking / ai-complete`
  - `provider = undefined`

If yes, then the root cause is provider detection failure, not rendering failure.

---

## 2. Verify whether Layer 1 or Layer 2 is missing Claude

Current architecture has two layers:

### Layer 1
Text/echo-based command detection in `pty.rs`

### Layer 2
Banner-based reconciliation in `process_monitor.rs`

We need to determine which one fails for the third Claude window.

### Exact questions

- Did Layer 1 detect the third Claude command from `output_since_enter`?
- If Layer 1 did not detect it, did Layer 2 later detect Claude from banner text?
- If Layer 2 also failed, what exact banner text was present in `recent_output_window`?

---

## 3. Compare Codex vs Claude banner strings from real output

Codex is successfully correcting to blue, so its fallback path is likely matching the real output.

Claude is not correcting in the third-window scenario, so we need the real banner text that Mini-Term actually sees.

### Required evidence

Capture the actual `recent_output_window` contents (or equivalent debug sample) for:

- the second-window Codex success case
- the third-window Claude failure case

Then compare them against `detect_provider_from_banner()` matching rules.

### Specific suspicion

Current Claude banner matching is probably not covering the exact runtime output that appears in this scenario, even though Codex matching does.

---

## 4. Verify whether recent_output_window really contains the expected Claude lines

Do not assume the visible terminal screen and `recent_output_window` are identical.

The user may visually see:

- `Claude Code v2.x.x`

but the backend window used by banner reconciliation may not contain the exact same line in a parseable form.

### Required questions

- Is the line `Claude Code v...` present in `recent_output_window`?
- If present, does it contain ANSI / carriage-return / overwrite patterns that prevent matching?
- If absent, why is it absent even though the user sees it rendered?

---

## 5. Verify whether the project-level blue dot is simply the surviving Codex provider

Project aggregation currently only counts panes with a provider.

So if:

- Codex pane = provider known (blue)
- third Claude pane = provider missing

then project-level result will remain blue.

### Required check

Confirm whether the visible blue project dot is coming solely from the Codex pane while the third Claude pane is excluded from aggregation because `aiProvider` is missing.

If yes, that explains the exact user perception.

---

## Suggested Temporary Instrumentation

Add minimal temporary logging only where needed.

### In `pty.rs`
Log for a target `pty_id`:

- when `track_input()` enters AI
- detected provider from direct token path
- detected provider from `output_since_enter`
- when `try_backfill_provider()` succeeds or fails

### In `process_monitor.rs`
Log for each monitored AI pane:

- `pty_id`
- `is_ai`
- `provider`
- whether `detect_provider_from_banner()` matched
- final emitted status/provider pair

### In frontend (`App.tsx` or store)
Log received `pty-status-change` payloads for the two/three panes involved.

The goal is to answer:

- did backend emit providerless AI status?
- or did frontend receive the right provider and still render wrong?

---

## Reproduction Script

Use exactly this order:

1. Open first pane
2. Launch Claude using right-arrow/history accept
3. Confirm orange appears
4. Open second pane
5. Launch Codex using right-arrow/history accept
6. Confirm blue appears
7. Open third pane
8. Launch Claude using right-arrow/history accept
9. Observe whether the third pane gets:
   - correct orange
   - gray/neutral
   - delayed correction
10. Capture logs immediately after step 9

Do not change shell, prompt theme, or command form mid-run.

---

## Success Criteria for This Investigation

This debug pass is complete only when we can answer all of the following:

- Does the third Claude pane become AI session internally?
- Does it have provider or not?
- If provider is missing, did Layer 1 fail, Layer 2 fail, or both?
- What exact banner/echo text was available when it failed?
- Is the visible blue project dot simply the surviving Codex provider from another pane?

Once those are known, a fix can be targeted instead of guessed.

---

## Instruction for Claude / Codex

Do not implement a blind fix yet.

Instead:

- reproduce the exact 3-window scenario
- collect backend and frontend evidence for `pty_id -> is_ai_session -> provider -> emitted status`
- compare successful Codex correction vs failing third-Claude correction
- identify whether the failure is in Layer 1 detection, Layer 2 banner reconciliation, or aggregation visibility
- report the concrete failing state transition before proposing code changes
