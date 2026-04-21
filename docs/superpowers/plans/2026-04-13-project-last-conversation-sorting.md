# Project Recent Conversation Sorting

## Goal

Make the project list easier to use by automatically moving projects that have recent AI conversations closer to the front.

This is **not** a real-time activity ranking feature.
Do **not** reorder projects based on changing runtime states like generating / awaiting input / complete.

The rule should be stable and simple:

- projects that were recently used for AI conversations should appear earlier
- projects with no recent AI conversations should appear later
- existing group structure must remain intact

---

## Core Requirement

Add a persistent timestamp field to each project:

- `lastConversationAt?: number`

This timestamp records the last time the user started or resumed an AI conversation for that project.

The project list should use this timestamp for sorting.

---

## Sorting Rules

### 1. Grouped projects

If projects are inside a group:

- sort only **within that group**
- do **not** move projects across groups
- do **not** reorder the groups themselves

Meaning:

- Group A keeps its position
- Group B keeps its position
- only the project order inside Group A or Group B changes

### 2. Ungrouped projects

Projects that are not inside any group should be sorted together in one shared top-level order.

Meaning:

- all top-level ungrouped projects are sorted by `lastConversationAt`
- grouped projects are not mixed into this top-level sorting

### 3. Group nodes are never sorted by conversation time

Only project nodes participate in recent-conversation sorting.

Group nodes:

- are not assigned `lastConversationAt`
- are not reordered by this feature

### 4. Timestamp sort direction

Sort by:

- `lastConversationAt` descending
- newer timestamps first

### 5. Projects with no conversation timestamp

Projects without `lastConversationAt` should be placed after projects that do have a timestamp.

For projects where both timestamps are missing:

- preserve existing manual/tree order

### 6. Stable ordering

If two projects have the same `lastConversationAt`:

- preserve their existing relative order

Do not introduce random jumping.

---

## When to Update `lastConversationAt`

Update the timestamp only when the user **starts** or **resumes** an AI conversation for the project.

### Must update on

- start Claude conversation
- start Codex conversation
- start Gemini conversation
- resume Claude conversation
- resume Codex conversation
- resume Gemini conversation

### Must NOT update on

- every output chunk
- every provider status change
- generating -> waiting -> complete transitions
- terminal focus changes
- ordinary shell commands unrelated to AI

This feature is intended to be stable, not highly dynamic.

---

## Product Intent

This feature should make the list feel like:

- "projects I actually use for AI work float forward over time"

It should **not** feel like:

- "the whole project list keeps jumping around while models are running"

---

## Example

Current tree:

- Group A
  - Project A1
  - Project A2
- Group B
  - Project B1
  - Project B2
- Project C1
- Project C2

Assume latest conversation times are:

- A2 = newest
- B1 = newest inside Group B
- C2 = newest top-level ungrouped
- C1 = older top-level ungrouped
- A1 = older inside Group A
- B2 = older inside Group B

Expected result:

- Group A
  - A2
  - A1
- Group B
  - B1
  - B2
- C2
- C1

Incorrect behavior would be:

- moving A2 out of Group A
- moving B1 above Group A
- reordering Group A and Group B themselves

---

## Data Model Changes

### Frontend types

Add to project config:

```ts
lastConversationAt?: number;
```

This field should be persisted with the project config.

### Persistence

Ensure the field survives:

- app restart
- config save/load
- layout save/load cycles

---

## Suggested Implementation Areas

Likely files to update:

- `src/types.ts`
  - add `lastConversationAt` to `ProjectConfig`
- config persistence files
  - ensure save/load includes the new field
- AI conversation start/resume hooks
  - update the owning project's `lastConversationAt`
- project tree rendering / ordering utilities
  - implement per-scope sorting logic
- `src/components/ProjectList.tsx`
  - consume sorted order without breaking existing tree structure

If there are existing project-tree helper utilities, prefer updating those instead of putting complex sorting logic directly in the component.

---

## Sorting Algorithm Requirements

Use the existing tree structure as the source of truth.

For each sibling list:

- keep group nodes in their original positions relative to other group nodes
- for project nodes under the same parent scope:
  - sort by `lastConversationAt` descending
  - missing timestamp goes last
  - equal timestamp keeps original order

Parent scope means:

- top-level ungrouped project list
- children list inside a specific group

Do not flatten the tree.

---

## Acceptance Criteria

### Basic behavior

- [ ] starting an AI conversation updates `lastConversationAt` for that project
- [ ] resuming an AI conversation updates `lastConversationAt` for that project
- [ ] restarting the app preserves the timestamp

### Group behavior

- [ ] projects inside a group sort only within that group
- [ ] groups themselves do not move
- [ ] projects never move across groups because of this feature

### Ungrouped behavior

- [ ] ungrouped top-level projects sort together by `lastConversationAt`
- [ ] grouped projects are not mixed into top-level sorting

### Stability

- [ ] runtime status changes do not continuously reorder the list
- [ ] equal timestamps preserve existing order
- [ ] missing timestamps go after projects with timestamps

### Regression safety

- [ ] drag-and-drop tree structure still works
- [ ] manual grouping still works
- [ ] project add/remove still works
- [ ] active project switching still works
- [ ] status dots and notifications are unaffected

---

## Non-Goals

Do not implement these in this task:

- sorting by live runtime state priority
- global cross-group ranking
- auto-pinning projects
- special "recent projects" section
- frequent re-sorting during model output

Keep the feature minimal and stable.

---

## Instruction for Claude / Codex

Implement exactly this behavior:

- add `lastConversationAt` to projects
- update it only when AI conversations are started or resumed
- sort projects by this field only within the same parent scope
- keep group positions unchanged
- keep ungrouped projects sorted together at top level
- avoid frequent dynamic reordering based on runtime state
