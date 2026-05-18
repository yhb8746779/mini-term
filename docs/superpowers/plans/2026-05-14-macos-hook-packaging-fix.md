# macOS Hook Packaging / Path Fix

## Goal

Fix the newly introduced AI hook integration so it actually works on macOS in the packaged `.app`, not just in an ad-hoc dev layout.

This document is for Claude/Codex to implement directly.

The scope is limited to the new hook system added in upstream commits around:

- `feat(hook): integrate Claude/Codex/Gemini hook event system`
- `fix(settings): correct hook toggle indicator positioning`

Do not refactor unrelated terminal/status code in this task.

---

## User-Visible Problem

After enabling the hook feature in Mini-Term settings on macOS, the registration UI may appear to succeed, but the actual hook callbacks can still fail at runtime in packaged app usage.

Observed risk profile:

1. the hook binary may not be bundled into the macOS `.app`
2. even if bundled, the generated command string may break because the app path contains spaces
3. even if the command runs, the hook helper's fallback port-file lookup can miss the real file because the app identifier/path assumptions do not match

That means the feature can look "implemented" while still being broken on real macOS installs.

---

## Confirmed Root Causes

## 1. `miniterm-hook` is declared as a Rust `[[bin]]`, but not configured as a bundled sidecar

Relevant files:

- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`
- `src-tauri/src/hook_registry.rs`

Current state:

- `Cargo.toml` defines `[[bin]] name = "miniterm-hook"`
- `hook_registry.rs` computes the hook binary path by taking `current_exe().parent().join("miniterm-hook")`
- but `tauri.conf.json` does not declare `bundle.externalBin`

Problem:

On macOS packaged apps, additional helper executables should not be assumed to sit next to the main app executable automatically. Tauri sidecar/external binary packaging must be configured explicitly.

So the current code can work in some local/dev layouts while failing in the packaged `.app`.

Reference:

- Tauri sidecar / external binary packaging docs: <https://v2.tauri.app/develop/sidecar/>

---

## 2. Non-Windows hook command generation does not quote the executable path

Relevant file:

- `src-tauri/src/hook_registry.rs`

Current code shape:

```rust
if cfg!(windows) {
    format!("\"{}\" {}", hook_path, event)
} else {
    format!("{} {}", hook_path, event)
}
```

Problem:

On macOS, packaged app paths commonly include spaces, for example:

```text
/Applications/Mini-Term.app/Contents/MacOS/miniterm-hook
```

or another path under a user-controlled folder with spaces.

Without quoting or a safer argv-style representation, the shell splits the path and the hook command fails before the helper even starts.

This is a direct bug and must be fixed even if packaging is corrected.

---

## 3. Hook port-file write path and read path use different app IDs

Relevant files:

- `src-tauri/tauri.conf.json`
- `src-tauri/src/hook_server.rs`
- `src-tauri/src/bin/miniterm-hook.rs`

Current state:

- app identifier in `tauri.conf.json` is `com.tauri-app.tauri-app`
- `hook_server.rs` writes `hook-server.json` through `app.path().app_data_dir()`
- `miniterm-hook.rs` fallback lookup hardcodes `com.mini-term.app`

Problem:

The helper writes/reads two different logical app-data locations.

So if `MINITERM_HOOK_PORT` is unavailable for any reason, the helper fallback lookup can silently fail on macOS because it reads:

```text
~/Library/Application Support/com.mini-term.app/hook-server.json
```

while the app likely wrote under the Tauri identifier-backed path for:

```text
com.tauri-app.tauri-app
```

This mismatch must be removed.

---

## Required Fix

Implement all three parts in one patch.

## Part A - Bundle the helper binary correctly for packaged builds

Update `src-tauri/tauri.conf.json` so Tauri bundles the hook helper as an external binary / sidecar.

Implementation requirement:

- use the Tauri-supported `bundle.externalBin` mechanism
- include the `miniterm-hook` helper in a way that works for macOS packaging
- do not rely on `current_exe().parent()` coincidentally containing the helper

Important:

- do not stop after adding `externalBin`
- the runtime lookup logic must also match the packaged location that Tauri uses for bundled helper binaries

If needed, switch runtime path resolution in `hook_registry.rs` from naive `current_exe().parent()` logic to the proper Tauri path resolution approach for sidecars/resources.

The implementation should be explicit and deterministic, not "try a few guesses and hope one exists".

---

## Part B - Always produce a safe command string for hook registration

Update all hook command builders in `src-tauri/src/hook_registry.rs`.

This includes:

- Claude hook entry builder
- Codex hook entry builder
- any manual snippet generator path that emits the same command form

Requirement:

- executable path must be safely quoted on macOS/Linux as well, not only on Windows
- event argument handling must remain correct

Minimum acceptable rule:

- if the target config format only accepts a string command, emit a safely shell-escaped command string

Do not keep the current "quote only on Windows" behavior.

---

## Part C - Unify the app-data identifier/path used by the hook helper fallback

There must be a single source of truth for where `hook-server.json` lives.

Required result:

- the server writes the port file to one canonical app-data location
- `miniterm-hook` reads from that same canonical location when `MINITERM_HOOK_PORT` is absent

Implementation options:

### Preferred option

Make the helper derive the exact same app-data location as the Tauri app uses, based on the real app identifier/config rather than a hardcoded unrelated string.

### Acceptable fallback

If sharing Tauri path resolution directly inside the helper is impractical, at minimum replace the hardcoded `com.mini-term.app` constant so it matches the actual app identifier used by the running application.

But the final code should avoid future drift as much as practical.

Important:

If you also decide to change the app identifier in `tauri.conf.json`, do so intentionally and verify all consequences. Do not casually rename identifiers unless necessary.

For this task, the safer default is:

- keep the current app identifier unchanged
- align hook helper lookup to it

---

## Files Expected To Change

At minimum:

- `src-tauri/tauri.conf.json`
- `src-tauri/src/hook_registry.rs`
- `src-tauri/src/bin/miniterm-hook.rs`

Possibly also:

- `src-tauri/build.rs`
- `src-tauri/src/lib.rs`
- any helper/path utility file if you create a shared app-id/path constant

---

## Suggested Implementation Notes

## 1. Avoid duplicated literals

Do not keep multiple independent hardcoded strings for:

- hook binary name
- app identifier / app-data lookup folder

If the same value is needed in multiple places, centralize it or derive it from one canonical source.

---

## 2. Keep the env-var fast path

This existing logic is still correct and should remain:

- PTY creation injects `MINITERM_HOOK_PORT`
- hook helper prefers that env var first

The file lookup path is only the fallback path.

Do not remove the env-var fast path.

---

## 3. Do not expand scope into a hook protocol redesign

This task is not asking for:

- changing the hook payload schema
- changing HTTP shape
- changing event names
- changing UI toggle behavior

Only fix packaging/path/command correctness.

---

## Validation

## Static validation

1. `npm run build`
2. `cd src-tauri && cargo test`
3. `cd src-tauri && cargo build`

If available on the machine:

4. `npm run tauri build`

The task is not complete if the code only type-checks in frontend and the macOS packaged path is still unverified.

---

## Runtime validation on macOS

Validate these exact scenarios.

### A. Hook registration command contains a safe executable path

After enabling/registering hooks:

- inspect generated Claude/Codex hook config
- confirm the emitted command still works when the Mini-Term app path contains spaces

Expected:

- path is properly quoted/escaped
- command launches helper successfully

### B. Packaged `.app` includes the helper binary

Build the macOS app bundle and inspect its contents.

Expected:

- `miniterm-hook` is actually bundled in the packaged app in the location assumed by runtime lookup

### C. Hook callback works via env-var path

Run Mini-Term, enable hook server, launch a supported AI tool from a Mini-Term PTY, and trigger a hook event.

Expected:

- helper can connect to local hook server
- event reaches Mini-Term

### D. Hook callback still works if fallback file lookup is needed

Simulate or verify the case where `MINITERM_HOOK_PORT` is absent and helper must read `hook-server.json`.

Expected:

- helper reads the same port file path the app writes
- callback still succeeds

---

## Acceptance Criteria

- [ ] `miniterm-hook` is bundled correctly for macOS packaged app usage
- [ ] runtime hook binary lookup matches the packaged location deterministically
- [ ] hook command strings are safe for paths containing spaces on macOS/Linux
- [ ] helper port-file fallback reads the same app-data location that the app writes
- [ ] no hardcoded mismatched app ID remains between writer and reader
- [ ] existing env-var fast path remains intact
- [ ] frontend build passes
- [ ] Rust test/build passes

---

## Instruction For Claude / Codex

Implement exactly this fix set:

1. fix bundled helper packaging for macOS packaged builds
2. fix hook command path quoting on non-Windows platforms
3. fix hook port-file path/app-id mismatch

Keep the patch tightly scoped to hook correctness.
