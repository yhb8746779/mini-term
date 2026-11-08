# 终端标签拖拽换位 实现计划

> 日期：2026-11-08 · 分支：`feat/terminal-drag`（基于 origin/main 0.2.56）

## 目标

右侧终端区域（TerminalArea）里的终端标签（PaneGroup 标签栏中的每个 pane tab）支持鼠标拖拽换位置：

1. **同组内重排**：在同一个 PaneGroup 标签栏内拖动 tab，改变顺序。
2. **跨组移动**：把 tab 拖到另一个 PaneGroup 的标签栏，移入该组（落在指定位置）。
3. **拖到终端区域边缘分屏**：把 tab 拖到某个 PaneGroup 的终端内容区，按落点区域决定：
   - 上/下/左/右 四个边缘区（各约 25%）→ 在目标组该方向新建分屏并把 pane 移过去；
   - 中央区 → 移入该组（追加到末尾并激活）。
4. 源组被拖空时自动从 SplitNode 树中移除，并正确折叠只剩一个子节点的 split。

只在当前 Tab（同一个 `TerminalTab.splitLayout` 树）内拖动，不做跨 Tab / 跨项目。

## 关键约束（已确认）

- 终端实例按 `ptyId` 缓存在 `src/utils/terminalCache.ts`（`getOrCreateTerminal`），PaneGroup 重挂载 `TerminalInstance` 时会重新 attach，**移动 pane 不会丢失终端内容，不需要也不允许 kill/重建 PTY**。
- `tauri.conf.json` 已 `dragDropEnabled: false`，HTML5 DnD 可用；项目已有 `src/utils/dragState.ts`（ProjectList 用）作为拖拽 payload 中转，复用它。
- `TerminalInstance.tsx` 已有文件拖入逻辑（`onDragOverCapture` / `onDropCapture`，从 FileTree 拖文件把路径写入 PTY）。**拖 pane tab 时不能触发文件拖入遮罩、不能把内容写进 PTY**。需要用 payload 类型区分：当 `getDragPayload()?.type === 'pane'` 时 TerminalInstance 的文件拖入 handler 直接 return（不 preventDefault，不显示遮罩），交给 PaneGroup 处理。
- 布局变化后需调用 `saveLayoutToConfig(projectId)` 持久化（与现有 `handleUpdateNode` 一致）。
- `SplitLayout.getNodeKey` 使用首个 pane id 作为 key，移动 pane 后 key 变化导致 Allotment 重挂载是可接受的。

## 实现步骤

### 1. `src/utils/dragState.ts`
扩展 `DragPayload` 联合类型：
```ts
| { type: 'pane'; paneId: string; sourceTabId: string }
```

### 2. 新增纯函数模块 `src/utils/splitTree.ts`
不依赖 React，便于单测：
- `findLeafByPaneId(node, paneId)`：返回包含该 pane 的 leaf。
- `removePaneFromTree(node, paneId): { tree: SplitNode | null; pane: PaneState | null }`
  - 从所在 leaf 移除 pane；若该 leaf 变空则删除该 leaf；
  - split 剩 1 个子节点时折叠成该子节点；剩多个时 sizes 按剩余子节点均分（与现有 `SplitLayout.handleChildClose` 行为一致）；
  - leaf 删除了当前 active pane 时，active 切到相邻 pane。
- `reorderPaneInLeaf(node, paneId, targetIndex)`：同组内重排。
- `insertPaneIntoLeaf(node, targetLeafFirstPaneId 或 leaf 标识, pane, index?)`：插入并激活。
- `splitLeafWithPane(node, targetLeafId, pane, edge: 'left'|'right'|'top'|'bottom')`：
  - left/right → direction `horizontal`，top/bottom → `vertical`；
  - left/top 新 leaf 放前，right/bottom 放后；sizes `[50, 50]`。
- `movePane(tree, paneId, target)`：组合以上操作，target 为
  `{ kind: 'reorder' | 'into', leafKey, index? } | { kind: 'edge', leafKey, edge }`。
  - 先定位目标 leaf（用目标 leaf 中某个**非被拖动**的 pane id 作为稳定标识，避免移除后找不到）；
  - 无操作情况（拖到自身位置、单 pane 组拖到自己的边缘/中央）直接返回原树。

leaf 标识建议：用目标 leaf 的 `activePaneId` 或其中一个不等于被拖 pane 的 pane id。

### 3. `src/components/PaneGroup.tsx`
- Props 新增：`tabId: string`、`onMovePane: (paneId: string, target: MoveTarget) => void`。
- 每个 tab div：`draggable`，`onDragStart` 中 `setDragPayload({ type: 'pane', paneId, sourceTabId })`，`e.dataTransfer.effectAllowed = 'move'`，并 `setData('application/x-mini-term-pane', paneId)`（**不要写 `text/plain`**，避免被当作文件路径）；`onDragEnd` 清空 payload 和指示状态。
- 标签栏 `onDragOver`：仅当 payload.type === 'pane' 时 `preventDefault`，按鼠标 X 与各 tab 中点计算插入索引，显示竖线插入指示（accent 色 2px）。`onDrop` → `onMovePane(paneId, { kind: 'reorder' 或 'into', ... , index })`。
- 终端内容区包一层覆盖：拖动 pane 期间（监听 payload 状态，可用组件 state `dropZone`）在内容区 `onDragOver` 计算落点区域（edge/center），渲染半透明高亮遮罩（例如右侧半区高亮表示"右侧分屏"，中央表示"移入"）。`onDrop` → `onMovePane`。遮罩 `pointer-events: none`，放在 TerminalInstance 之上。
- 处理 `onDragLeave` 用 `relatedTarget` 判断真正离开，和 TerminalInstance 现有写法一致。
- 拖动中给源 tab 降低透明度。

### 4. `src/components/SplitLayout.tsx`
透传 `tabId`、`onMovePane` 到递归子节点和 PaneGroup。

### 5. `src/components/TerminalArea.tsx`
- 实现 `handleMovePane(paneId, target)`：从 store 读当前 active tab，调用 `movePane`，结果非 null 则 `updateTabLayout` + `saveLayoutToConfig`；若 payload.sourceTabId 与当前 tab 不一致则忽略。
- 传给 SplitLayout。

### 6. `src/components/TerminalInstance.tsx`
在 `handleDragOver` / `handleDragLeave` / `handleDrop` 开头加判断：`if (getDragPayload()?.type === 'pane') return;`，确保 pane 拖拽不触发文件拖入。

### 7. 测试
- 沿用项目现有纯函数测试约定：把 `src/utils/splitTree.ts` 加入 `tsconfig.test.json` 的 `include`，新增 `tests/splitTree.test.cjs`（`node:assert/strict`，`require('../.tmp-tests/splitTree.js')`，参考 `tests/ptyWriteQueue.test.cjs`）。**splitTree.ts 只能 `import type`，不能引入运行时依赖**（否则 CommonJS 编译产物跑不起来）。运行：`npx tsc -p tsconfig.test.json && node tests/splitTree.test.cjs`。覆盖：同组重排、跨组移动、拖空源组折叠 split、边缘分屏四方向、无操作返回原树、activePaneId 正确。**不新增依赖**。
- `npm run build`（`tsc && vite build`）必须通过。

## 手动验收用例
1. 一个组内 3 个 tab，拖第 3 个到第 1 个前 → 顺序变化，终端内容保留。
2. 左右分屏，把左组唯一 tab 拖到右组标签栏 → 左组消失，变回单组，含 2 个 tab。
3. 把 tab 拖到另一组内容区的下边缘 → 该组上下分屏，新组含该 tab。
4. 拖 pane 经过终端区域时不出现"文件拖入"遮罩，松手不往终端写字符；从文件树拖文件到终端仍正常写入路径。
5. 重启 app 后布局按拖拽后的结果恢复。

## 不做
- 跨 Tab / 跨项目拖动
- 拖出成新窗口
