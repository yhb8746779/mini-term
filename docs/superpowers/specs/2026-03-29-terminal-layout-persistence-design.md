# Terminal Layout Persistence Design

Persist terminal layout (tabs, splits, pane shell assignments) per project, and restore it on next open.

## Data Model

### Serialization Types (new)

Runtime `SplitNode` contains ephemeral data (`ptyId`, `status`, `id`). A parallel set of "saved" types strips these down to structure-only:

```typescript
interface SavedPane {
  shellName: string;
}

type SavedSplitNode =
  | { type: 'leaf'; pane: SavedPane }
  | { type: 'split'; direction: 'horizontal' | 'vertical'; children: SavedSplitNode[]; sizes: number[] };

interface SavedTab {
  customTitle?: string;
  splitLayout: SavedSplitNode;
}

interface SavedProjectLayout {
  tabs: SavedTab[];
  activeTabIndex: number;
}
```

### ProjectConfig Extension

`ProjectConfig` gains an optional `savedLayout` field:

```typescript
interface ProjectConfig {
  id: string;
  name: string;
  path: string;
  savedLayout?: SavedProjectLayout;
}
```

Rust side mirrors this with `Option<SavedProjectLayout>` on `ProjectConfig`, using `#[serde(rename_all = "camelCase")]`.

## Serialization (Save)

A pure function `serializeLayout(ps: ProjectState): SavedProjectLayout` in `store.ts`:

1. Iterates `ps.tabs`, for each tab recursively walks the `SplitNode` tree
2. At each leaf, extracts `shellName` only (drops `id`, `ptyId`, `status`)
3. At each split, preserves `direction`, `children`, `sizes`
4. Records `activeTabIndex = tabs.findIndex(t => t.id === ps.activeTabId)`

## Deserialization (Restore)

An async function `restoreLayout(projectId, savedLayout, projectPath, shells)` in `store.ts`:

1. Iterates `savedLayout.tabs`, for each `SavedTab` recursively walks `SavedSplitNode`
2. At each leaf:
   - Finds shell config by matching `shellName` against `config.availableShells[].name`
   - Falls back to `config.defaultShell` if not found
   - Calls `invoke('create_pty', { shell, args, cwd: projectPath })` to get a `ptyId`
   - Assembles `PaneState { id: genId(), shellName, status: 'idle', ptyId }`
3. Assembles `TerminalTab { id: genId(), splitLayout, status: 'idle', customTitle }`
4. Sets `activeTabId = tabs[savedLayout.activeTabIndex]?.id ?? tabs[0]?.id`

### Error Handling

- Single pane PTY creation failure: skip the pane, remove from tree (reuse `removePane`)
- All panes in a tab fail: skip the tab
- All tabs fail: degrade to empty tabs (as if no savedLayout)
- Shell name not found in availableShells: fallback to defaultShell
- Silent degradation, no user-facing error

## Save Triggers

Reuse existing `save_config` + 500ms debounce. A new `saveLayoutToConfig()` method:

1. Calls `serializeLayout()` on current project's `ProjectState`
2. Writes result into `ProjectConfig.savedLayout`
3. Calls `invoke('save_config', { config })` (debounced)

Trigger points (all in existing code paths):

| Operation | Location | Hook |
|-----------|----------|------|
| New tab | `TerminalArea.tsx` `handleNewTab` | After addTab |
| Split pane | `TerminalArea.tsx` `handleSplitPane` | After insertSplit |
| Close pane | `TerminalArea.tsx` `handleClosePane` | After removePane |
| Close tab | `TerminalArea.tsx` `handleCloseTab` | After tab removal |
| Drag tab to split | `TerminalArea.tsx` `handleTabDrop` | After layout update |
| Resize splits | `SplitLayout.tsx` Allotment `onChange` | In callback |

## Restore Timing

In `App.tsx` initialization `useEffect`, after `load_config`:

```
for each project in config.projects:
  if project.savedLayout exists and has tabs:
    await restoreLayout(project.id, project.savedLayout, project.path, config)
  else:
    initialize with empty tabs (current behavior)
```

## Files Changed

| File | Change |
|------|--------|
| `src/types.ts` | Add `SavedPane`, `SavedSplitNode`, `SavedTab`, `SavedProjectLayout`; extend `ProjectConfig` |
| `src-tauri/src/config.rs` | Add corresponding Rust structs; extend `ProjectConfig` |
| `src/store.ts` | Add `serializeLayout()`, `restoreLayout()`, `saveLayoutToConfig()` |
| `src/App.tsx` | Call `restoreLayout` during init |
| `src/components/TerminalArea.tsx` | Call `saveLayoutToConfig` after layout mutations |
| `src/components/SplitLayout.tsx` | Call `saveLayoutToConfig` on Allotment resize |

## Not Changed

- No new Tauri commands (reuses `save_config` / `load_config` / `create_pty`)
- No new storage files (reuses `config.json`)
- No changes to PTY lifecycle management
