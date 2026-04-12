# AI 完成提醒实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** AI 任务结束时通过三层提醒（项目列表 DONE tag、右下角 toast、Windows 任务栏闪烁）让用户更容易注意到，避免错过完成时机。

**Architecture:** 在 `store.ts` 的 `updatePaneStatusByPty` 内集中检测 `ai-working → ai-idle` 状态变化，触发三个独立的反应路径：（1）设置 `ProjectState.needsAttention` 持久标志驱动 `<DoneTag>` 渲染；（2）push 到 `notifications` 切片驱动右下角 `<ToastContainer>`；（3）调用 Tauri `requestUserAttention(Informational)` API 让 OS 处理任务栏闪烁。Tag/Toast 仅对非激活项目触发；任务栏闪烁不区分激活项目（Tauri API 自带 focus 检测）。

**Tech Stack:** React 19 + TypeScript + Zustand store + Tauri v2 (`@tauri-apps/api/window`) + Tailwind CSS v4 自定义样式。无新依赖。

**Spec:** `docs/superpowers/specs/2026-04-10-ai-completion-notification-design.md`

## 文件结构

**修改 (7 个文件):**
- `src-tauri/src/config.rs` — 加 2 个 bool 字段 + 2 个 default helper
- `src/types.ts` — 加 `AiCompletionNotification` interface；扩展 `ProjectState` 和 `AppConfig`
- `src/store.ts` — 加 Tauri import；加 `notifications` 切片 + push/dismiss 方法；改 `updatePaneStatusByPty` / `setActiveProject` / `removeProject`；加 `findPaneByPty` helper；扩展初始 config
- `src/components/ProjectList.tsx` — `renderProjectItem` 中条件渲染 `<DoneTag>` 替代 `<StatusDot>`
- `src/components/SettingsModal.tsx` — `SystemSettings` 加两个 toggle 行
- `src/styles.css` — 加 `.done-tag`、`.toast-*` CSS 和 `tagFadeIn`、`toastSlideIn` keyframes
- `src/App.tsx` — 顶层挂载 `<ToastContainer />`

**新建 (2 个文件):**
- `src/components/DoneTag.tsx` — 单行函数组件
- `src/components/ToastContainer.tsx` — 订阅 store 渲染 toast 栈，自管 5s 计时

---

### Task 1: Rust 后端 — AppConfig 加配置字段

**Files:**
- Modify: `src-tauri/src/config.rs:34-56` (struct 字段)
- Modify: `src-tauri/src/config.rs:117-128` (helper 函数区)
- Modify: `src-tauri/src/config.rs:130-147` (Default impl)

- [ ] **Step 1: 在 AppConfig struct 末尾添加两个字段**

在 `src-tauri/src/config.rs` 第 55 行（`pub terminal_follow_theme: bool,` 之后）追加：

```rust
    #[serde(default = "default_ai_completion_popup")]
    pub ai_completion_popup: bool,
    #[serde(default = "default_ai_completion_taskbar_flash")]
    pub ai_completion_taskbar_flash: bool,
```

- [ ] **Step 2: 添加两个 default helper 函数**

在 `src-tauri/src/config.rs` 第 128 行（`default_terminal_follow_theme` 函数之后，`impl Default for AppConfig` 之前）追加：

```rust
fn default_ai_completion_popup() -> bool {
    true
}
fn default_ai_completion_taskbar_flash() -> bool {
    true
}
```

- [ ] **Step 3: 在 Default impl 中初始化新字段**

修改 `impl Default for AppConfig`（约 130-147 行），在 `terminal_follow_theme: default_terminal_follow_theme(),` 之后追加：

```rust
            ai_completion_popup: default_ai_completion_popup(),
            ai_completion_taskbar_flash: default_ai_completion_taskbar_flash(),
```

- [ ] **Step 4: 验证 Rust 编译**

Run: `cd src-tauri && cargo check 2>&1`
Expected: `Finished` 且无 error。warning 可接受。

- [ ] **Step 5: 提交**

```bash
cd D:/Git/mini-term && git add src-tauri/src/config.rs && git commit -m "$(cat <<'EOF'
feat: AppConfig 增加 AI 完成提醒配置字段
- 新增 ai_completion_popup 控制右下角弹框开关，默认开启
- 新增 ai_completion_taskbar_flash 控制 Windows 任务栏闪烁开关，默认开启
- 配套新增 default_ai_completion_popup 和 default_ai_completion_taskbar_flash helper
EOF
)"
```

---

### Task 2: TypeScript 类型扩展

**Files:**
- Modify: `src/types.ts:12-26` (`AppConfig` interface)
- Modify: `src/types.ts:66-70` (`ProjectState` interface)
- Modify: `src/types.ts:62` 附近 (新增 `AiCompletionNotification`)

- [ ] **Step 1: 扩展 AppConfig**

在 `src/types.ts` 第 25 行（`terminalFollowTheme: boolean;` 之后）追加：

```typescript
  aiCompletionPopup: boolean;
  aiCompletionTaskbarFlash: boolean;
```

- [ ] **Step 2: 扩展 ProjectState 加 needsAttention**

修改 `src/types.ts` 第 66-70 行：

```typescript
export interface ProjectState {
  id: string;
  tabs: TerminalTab[];
  activeTabId: string;
  needsAttention?: boolean;
}
```

- [ ] **Step 3: 新增 AiCompletionNotification interface**

在 `src/types.ts` 第 70 行（`ProjectState` interface 之后、`TerminalTab` interface 之前）插入：

```typescript
export interface AiCompletionNotification {
  id: string;
  projectId: string;
  projectName: string;
  timestamp: number;
}

```

- [ ] **Step 4: 验证 TypeScript 编译**

Run: `cd D:/Git/mini-term && npx tsc --noEmit 2>&1`
Expected: 无输出（成功）或仅有与本任务无关的预存错误。**不能引入新错误**。

> 如果出现 `Property 'aiCompletionPopup' is missing in type` 之类错误，是因为 store 初始 config 还没加新字段——这是 Task 3 Step 3 的工作。为了让 Task 2 的 commit 也能编译通过，跳到 Task 3 Step 3（"扩展初始 config 包含两个新字段"）先做完，再把 Task 2 + Task 3 Step 3 的改动合并到同一个 commit。或者直接按顺序连续完成 Task 2 + 整个 Task 3 后再 commit。

- [ ] **Step 5: 提交（可能与 Task 3 Step 3 合并）**

```bash
cd D:/Git/mini-term && git add src/types.ts && git commit -m "$(cat <<'EOF'
feat: 扩展前端类型定义支持 AI 完成提醒
- AppConfig 新增 aiCompletionPopup 和 aiCompletionTaskbarFlash 字段
- ProjectState 新增 needsAttention 可选字段
- 新增 AiCompletionNotification 接口定义 toast 通知数据结构
EOF
)"
```

---

### Task 3: store.ts — notifications 切片 + 初始 config

**Files:**
- Modify: `src/store.ts:1-16` (types import)
- Modify: `src/store.ts:291-319` (`AppStore` interface)
- Modify: `src/store.ts:321-331` (初始 state)

> **依赖说明：** Tauri window API 的 import 不在此 Task 添加（会因 `noUnusedLocals: true` 触发未使用编译错误），统一在 Task 4 Step 1 添加（与首次使用同步）。

- [ ] **Step 1: 在 types import 中加入 AiCompletionNotification**

在 `src/store.ts` 第 3-16 行的 types import 块中加入 `AiCompletionNotification`：

```typescript
import type {
  AppConfig,
  ProjectConfig,
  ProjectGroup,
  ProjectState,
  TerminalTab,
  SplitNode,
  PaneState,
  PaneStatus,
  SavedPane,
  SavedSplitNode,
  SavedTab,
  SavedProjectLayout,
  AiCompletionNotification,
} from './types';
```

- [ ] **Step 2: 扩展 AppStore interface**

在 `src/store.ts` 第 311 行（`updatePaneStatusByPty` 声明）之后、第 314 行（`createGroup` 声明）之前插入：

```typescript
  // Notifications
  notifications: AiCompletionNotification[];
  pushNotification: (n: Omit<AiCompletionNotification, 'id' | 'timestamp'>) => void;
  dismissNotification: (id: string) => void;
```

- [ ] **Step 3: 扩展初始 config 包含两个新字段**

修改 `src/store.ts` 第 322-330 行的初始 config object：

```typescript
  config: {
    projects: [],
    defaultShell: '',
    availableShells: [],
    uiFontSize: 13,
    terminalFontSize: 14,
    theme: 'auto',
    terminalFollowTheme: true,
    aiCompletionPopup: true,
    aiCompletionTaskbarFlash: true,
  },
```

- [ ] **Step 4: 添加初始 notifications 数组**

在 `src/store.ts` 约第 334 行（`projectStates: new Map(),` 之后）插入：

```typescript
  notifications: [],
```

- [ ] **Step 5: 实现 pushNotification 和 dismissNotification**

在 `src/store.ts` `updatePaneStatusByPty` 实现之后（约第 464 行）、`createGroup` 实现之前插入：

```typescript
  pushNotification: (n) =>
    set((state) => ({
      notifications: [
        ...state.notifications,
        { ...n, id: genId(), timestamp: Date.now() },
      ],
    })),

  dismissNotification: (id) =>
    set((state) => ({
      notifications: state.notifications.filter((x) => x.id !== id),
    })),
```

- [ ] **Step 6: 验证 TypeScript 编译**

Run: `cd D:/Git/mini-term && npx tsc --noEmit 2>&1`
Expected: 无新错误

- [ ] **Step 7: 提交**

```bash
cd D:/Git/mini-term && git add src/store.ts && git commit -m "$(cat <<'EOF'
feat: store 新增 notifications 切片支持 AI 完成 toast
- 引入 AiCompletionNotification 类型并在 AppStore 中暴露 notifications 数组
- 实现 pushNotification 和 dismissNotification 操作方法
- 初始 config 补充 aiCompletionPopup 和 aiCompletionTaskbarFlash 默认值
EOF
)"
```

---

### Task 4: store.ts — transition 检测 + 清除逻辑

**Files:**
- Modify: `src/store.ts:1-2` 附近 (Tauri import)
- Modify: `src/store.ts:72` 之后 (新增 `findPaneByPty` helper)
- Modify: `src/store.ts:336` (`setActiveProject`)
- Modify: `src/store.ts:356-376` (`removeProject`)
- Modify: `src/store.ts:436-464` (`updatePaneStatusByPty`)

- [ ] **Step 1: 添加 Tauri window API import 和 findPaneByPty helper**

**1a. 添加 Tauri import：** 在 `src/store.ts` 第 1-2 行附近（紧跟 `import { create } from 'zustand';` 和 `import { invoke } from '@tauri-apps/api/core';` 之后）追加一行：

```typescript
import { getCurrentWindow, UserAttentionType } from '@tauri-apps/api/window';
```

**1b. 添加 findPaneByPty helper：** 在 `src/store.ts` 第 72 行（`collectPtyIds` 函数最后一个 `}` 之后）插入空行 + 函数：

```typescript

// 查找 ptyId 所属的 pane（按 SplitNode 树深搜）
function findPaneByPty(node: SplitNode, ptyId: number): PaneState | null {
  if (node.type === 'leaf') {
    return node.panes.find((p) => p.ptyId === ptyId) ?? null;
  }
  for (const child of node.children) {
    const found = findPaneByPty(child, ptyId);
    if (found) return found;
  }
  return null;
}
```

- [ ] **Step 2: 重写 updatePaneStatusByPty 实现**

完整替换 `src/store.ts` 第 436-464 行 `updatePaneStatusByPty` 的现有实现：

```typescript
  updatePaneStatusByPty: (ptyId, status) =>
    set((state) => {
      // 1. 找到 pane 所属项目并捕获 oldStatus
      let oldStatus: PaneStatus | null = null;
      let owningProjectId: string | null = null;
      for (const [pid, ps] of state.projectStates) {
        for (const tab of ps.tabs) {
          const found = findPaneByPty(tab.splitLayout, ptyId);
          if (found) {
            oldStatus = found.status;
            owningProjectId = pid;
            break;
          }
        }
        if (owningProjectId) break;
      }
      if (!owningProjectId || oldStatus === null) return state;

      // 2. 更新各项目 tabs 中匹配 ptyId 的 pane status
      const newStates = new Map(state.projectStates);
      let changed = false;
      for (const [pid, ps] of newStates) {
        let tabsChanged = false;
        const updatedTabs = ps.tabs.map((tab) => {
          const newLayout = updatePaneStatus(tab.splitLayout, ptyId, status);
          if (newLayout === tab.splitLayout) return tab;
          tabsChanged = true;
          return { ...tab, splitLayout: newLayout, status: getHighestStatus(newLayout) };
        });
        if (tabsChanged) {
          newStates.set(pid, { ...ps, tabs: updatedTabs });
          changed = true;
        }
      }
      if (!changed) return state;

      // 3. 检测 transition：ai-working → ai-idle
      const isCompletion = oldStatus === 'ai-working' && status === 'ai-idle';
      if (isCompletion) {
        // 3a. 任务栏闪烁 — 不区分激活项目（Tauri API 自带 focus 检测）
        if (state.config.aiCompletionTaskbarFlash) {
          queueMicrotask(() => {
            getCurrentWindow()
              .requestUserAttention(UserAttentionType.Informational)
              .catch(() => {});
          });
        }

        // 3b. Tag + Toast — 仅非激活项目
        if (owningProjectId !== state.activeProjectId) {
          const ps = newStates.get(owningProjectId);
          if (ps && !ps.needsAttention) {
            // 设置 needsAttention（防重：已为 true 时不重复）
            newStates.set(owningProjectId, { ...ps, needsAttention: true });

            // 推 toast（同项目当前没有未消失的 toast 才推）
            if (state.config.aiCompletionPopup) {
              const project = state.config.projects.find((p) => p.id === owningProjectId);
              const hasExisting = state.notifications.some(
                (n) => n.projectId === owningProjectId
              );
              if (project && !hasExisting) {
                const projectName = project.name;
                const targetPid = owningProjectId;
                queueMicrotask(() =>
                  useAppStore.getState().pushNotification({
                    projectId: targetPid,
                    projectName,
                  })
                );
              }
            }
          }
        }
      }

      return { projectStates: newStates };
    }),
```

- [ ] **Step 3: 修改 setActiveProject 加清除逻辑**

替换 `src/store.ts` 第 336 行的 `setActiveProject` 实现：

```typescript
  setActiveProject: (id) =>
    set((state) => {
      const newStates = new Map(state.projectStates);
      const ps = newStates.get(id);
      if (ps?.needsAttention) {
        newStates.set(id, { ...ps, needsAttention: false });
      }
      return { activeProjectId: id, projectStates: newStates };
    }),
```

- [ ] **Step 4: 修改 removeProject 清理 notifications**

修改 `src/store.ts` 第 356-376 行 `removeProject` 实现，在 return 语句中加 `notifications` 字段。完整新版本：

```typescript
  removeProject: (id) =>
    set((state) => {
      expandedDirsMap.delete(id);
      const timer = saveExpandedTimers.get(id);
      if (timer) { clearTimeout(timer); saveExpandedTimers.delete(id); }

      const newTree = deepCloneTree(state.config.projectTree ?? []);
      removeProjectFromTree(newTree, id);
      const newConfig = {
        ...state.config,
        projects: state.config.projects.filter((p) => p.id !== id),
        projectTree: newTree,
      };
      const newStates = new Map(state.projectStates);
      newStates.delete(id);
      const newActive =
        state.activeProjectId === id
          ? newConfig.projects[0]?.id ?? null
          : state.activeProjectId;
      return {
        config: newConfig,
        projectStates: newStates,
        activeProjectId: newActive,
        notifications: state.notifications.filter((n) => n.projectId !== id),
      };
    }),
```

- [ ] **Step 5: 验证 TypeScript 编译**

Run: `cd D:/Git/mini-term && npx tsc --noEmit 2>&1`
Expected: 无新错误

- [ ] **Step 6: 提交**

```bash
cd D:/Git/mini-term && git add src/store.ts && git commit -m "$(cat <<'EOF'
feat: store 新增 AI 完成 transition 检测和清除逻辑
- updatePaneStatusByPty 在 ai-working→ai-idle 转换时设置 needsAttention 并推 toast
- 任务栏闪烁通过 requestUserAttention 触发，不区分激活项目（Tauri API 自带 focus 判断）
- Tag 和 toast 仅对非激活项目触发，防止干扰用户当前正在浏览的项目
- setActiveProject 在激活项目时清除 needsAttention 标志
- removeProject 同步清理 notifications 中该项目的所有未消失通知
- 新增 findPaneByPty helper 用于在 SplitNode 树中查找 pane
EOF
)"
```

---

### Task 5: DoneTag 组件 + Tag CSS + ProjectList 集成

**Files:**
- Create: `src/components/DoneTag.tsx`
- Modify: `src/styles.css:160` 附近 (Animations 区追加)
- Modify: `src/components/ProjectList.tsx:7` (import)
- Modify: `src/components/ProjectList.tsx:341` (条件渲染)

- [ ] **Step 1: 创建 DoneTag 组件**

新建 `src\components\DoneTag.tsx`，内容：

```tsx
export function DoneTag() {
  return <span className="done-tag">DONE</span>;
}
```

- [ ] **Step 2: 在 styles.css 追加 Tag 样式**

在 `src/styles.css` 第 176 行（`.animate-slide-in` 定义之后、`/* ===== Context menu ===== */` 之前）追加：

```css
/* ===== AI 完成 Tag ===== */
.done-tag {
  display: inline-flex;
  align-items: center;
  padding: 2px 8px;
  background: var(--color-success);
  color: var(--bg-base);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.06em;
  border-radius: 10px;
  font-family: system-ui, sans-serif;
  box-shadow: 0 0 0 1px rgba(107, 184, 122, 0.4),
              0 0 8px rgba(107, 184, 122, 0.3);
  animation: tagFadeIn 0.3s ease-out;
}

:root[data-theme="light"] .done-tag {
  color: #ffffff;
  box-shadow: 0 0 0 1px rgba(45, 138, 70, 0.4),
              0 1px 3px rgba(45, 138, 70, 0.3);
}

@keyframes tagFadeIn {
  0% { opacity: 0; transform: scale(0.6); }
  60% { opacity: 1; transform: scale(1.15); }
  100% { transform: scale(1); }
}
```

- [ ] **Step 3: 在 ProjectList.tsx 导入 DoneTag**

在 `src/components/ProjectList.tsx` 第 8 行（`import { StatusDot } from './StatusDot';` 之后）追加：

```tsx
import { DoneTag } from './DoneTag';
```

- [ ] **Step 4: 在 renderProjectItem 中条件渲染**

修改 `src/components/ProjectList.tsx` 第 341 行 `<StatusDot status={projectStatus} />`。需要先获取 `ps` 和 `isActive`：

`isActive` 已在第 271 行定义。在第 273 行（`const projectStatus = getProjectStatus(project.id);` 之后）插入：

```tsx
    const projectPs = projectStates.get(project.id);
    const showDoneTag = !!projectPs?.needsAttention && !isActive;
```

然后将第 341 行：

```tsx
        <StatusDot status={projectStatus} />
```

替换为：

```tsx
        {showDoneTag ? <DoneTag /> : <StatusDot status={projectStatus} />}
```

- [ ] **Step 5: 验证 TypeScript 编译**

Run: `cd D:/Git/mini-term && npx tsc --noEmit 2>&1`
Expected: 无新错误

- [ ] **Step 6: 启动应用快速验证 Tag 渲染**

Run: `cd D:/Git/mini-term && npm run tauri dev` （在另一个终端运行，长时间执行）

手动验证：
- 切换到一个非激活项目，运行 `claude` 命令
- 切换到别的项目（让该项目变非激活）
- 等 AI 跑完
- 该项目末尾应该出现绿色 "DONE" 胶囊（带微光晕，浅色和深色模式都正确）
- 点击该项目（激活它）→ DONE 胶囊消失，恢复为 StatusDot 的圆点

确认 OK 后停止 dev server（Ctrl+C）。

- [ ] **Step 7: 提交**

```bash
cd D:/Git/mini-term && git add src/components/DoneTag.tsx src/styles.css src/components/ProjectList.tsx && git commit -m "$(cat <<'EOF'
feat: 项目列表新增 AI 完成 DONE tag
- 新建 DoneTag 组件渲染绿色实心胶囊 + 微光晕
- 样式使用 var(--color-success) 自动适配深浅色主题
- ProjectList 在 needsAttention 为 true 且非激活项目时用 DoneTag 替代 StatusDot
- 新增 tagFadeIn keyframes 实现 0.3s 缩放进场动画
EOF
)"
```

---

### Task 6: ToastContainer 组件 + Toast CSS + App.tsx 挂载

**Files:**
- Create: `src/components/ToastContainer.tsx`
- Modify: `src/styles.css` (Tag 样式之后追加)
- Modify: `src/App.tsx:13` (import)
- Modify: `src/App.tsx:214` (JSX 挂载)

- [ ] **Step 1: 创建 ToastContainer 组件**

新建 `src\components\ToastContainer.tsx`，内容：

```tsx
import { useEffect } from 'react';
import { useAppStore } from '../store';

export function ToastContainer() {
  const notifications = useAppStore((s) => s.notifications);
  const dismissNotification = useAppStore((s) => s.dismissNotification);
  const setActiveProject = useAppStore((s) => s.setActiveProject);

  // 每个 notification 5s 后自动消失
  useEffect(() => {
    const timers = notifications.map((n) =>
      setTimeout(() => dismissNotification(n.id), 5000)
    );
    return () => timers.forEach(clearTimeout);
  }, [notifications, dismissNotification]);

  // 最多同时渲染 5 个，超出排队
  const visible = notifications.slice(0, 5);

  if (visible.length === 0) return null;

  return (
    <div className="toast-stack">
      {visible.map((n) => (
        <div
          key={n.id}
          className="toast-card"
          onClick={() => {
            setActiveProject(n.projectId);
            dismissNotification(n.id);
          }}
        >
          <div className="toast-icon">✓</div>
          <div className="toast-body">
            <div className="toast-name">{n.projectName}</div>
            <div className="toast-desc">AI 已完成 · 点击查看</div>
          </div>
          <div
            className="toast-close"
            onClick={(e) => {
              e.stopPropagation();
              dismissNotification(n.id);
            }}
          >×</div>
        </div>
      ))}
    </div>
  );
}
```

- [ ] **Step 2: 在 styles.css 追加 Toast 样式**

在 `src/styles.css` 中 `@keyframes tagFadeIn` 定义之后追加：

```css
/* ===== AI 完成 Toast ===== */
.toast-stack {
  position: fixed;
  right: 16px;
  bottom: 16px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  z-index: 70;
  pointer-events: none;
}

.toast-card {
  width: 280px;
  background: var(--bg-elevated);
  border: 1px solid var(--border-default);
  border-left: 3px solid var(--color-success);
  border-radius: 6px;
  padding: 10px 12px;
  display: flex;
  align-items: center;
  gap: 10px;
  box-shadow: var(--shadow-overlay);
  font-family: system-ui, sans-serif;
  cursor: pointer;
  pointer-events: auto;
  animation: toastSlideIn 0.25s ease-out;
  transition: transform 0.15s;
}

.toast-card:hover { transform: translateX(-2px); }

.toast-icon {
  width: 20px;
  height: 20px;
  border-radius: 50%;
  background: var(--color-success);
  color: var(--bg-base);
  display: flex;
  align-items: center;
  justify-content: center;
  font-weight: 700;
  font-size: 11px;
  flex-shrink: 0;
}

.toast-body { flex: 1; min-width: 0; }
.toast-name {
  color: var(--text-primary);
  font-size: 12px;
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.toast-desc {
  color: var(--text-secondary);
  font-size: 10px;
  margin-top: 1px;
}
.toast-close {
  color: var(--text-muted);
  font-size: 14px;
  padding: 0 4px;
  cursor: pointer;
}
.toast-close:hover { color: var(--text-primary); }

:root[data-theme="light"] .toast-icon { color: #ffffff; }

@keyframes toastSlideIn {
  from { opacity: 0; transform: translateX(100%); }
  to { opacity: 1; transform: translateX(0); }
}
```

- [ ] **Step 3: 在 App.tsx 导入 ToastContainer**

在 `src/App.tsx` 第 13 行（`import { SettingsModal } from './components/SettingsModal';` 之后）追加：

```tsx
import { ToastContainer } from './components/ToastContainer';
```

- [ ] **Step 4: 在 App.tsx 挂载 ToastContainer**

修改 `src/App.tsx` 第 214 行附近，在 `<SettingsModal>` 之后、最外层 `</div>` 之前插入：

```tsx
      <SettingsModal open={configOpen} onClose={() => setConfigOpen(false)} />
      <ToastContainer />
    </div>
  );
}
```

- [ ] **Step 5: 验证 TypeScript 编译**

Run: `cd D:/Git/mini-term && npx tsc --noEmit 2>&1`
Expected: 无新错误

- [ ] **Step 6: 提交**

```bash
cd D:/Git/mini-term && git add src/components/ToastContainer.tsx src/styles.css src/App.tsx && git commit -m "$(cat <<'EOF'
feat: 新增 AI 完成 toast 通知组件
- 新建 ToastContainer 组件订阅 store.notifications，渲染右下角紧凑卡片栈
- 自管 5s 自动消失，最多同时渲染 5 个，溢出排队
- 整张卡片点击触发 setActiveProject 跳转到对应项目并 dismiss
- × 按钮仅 dismiss 单个 toast，不影响目标项目的 needsAttention
- 样式使用 var(--color-success) 自动适配深浅色主题，从右侧滑入动画
- 在 App.tsx 顶层 fixed 挂载，z-index 70 浮于主布局之上
EOF
)"
```

---

### Task 7: SettingsModal 添加配置 toggle

**Files:**
- Modify: `src/components/SettingsModal.tsx:336-341` (handler 区追加)
- Modify: `src/components/SettingsModal.tsx:388` (toggle JSX 追加)

- [ ] **Step 1: 添加两个 handler 函数**

在 `src/components/SettingsModal.tsx` 第 341 行（`handleTerminalFollowThemeChange` 之后）追加：

```tsx
  const handleAiCompletionPopupChange = useCallback((enabled: boolean) => {
    const newConfig = { ...useAppStore.getState().config, aiCompletionPopup: enabled };
    setConfig(newConfig);
    invoke('save_config', { config: newConfig });
  }, [setConfig]);

  const handleAiCompletionTaskbarFlashChange = useCallback((enabled: boolean) => {
    const newConfig = { ...useAppStore.getState().config, aiCompletionTaskbarFlash: enabled };
    setConfig(newConfig);
    invoke('save_config', { config: newConfig });
  }, [setConfig]);
```

- [ ] **Step 2: 在 JSX 中添加两个 toggle 行**

在 `src/components/SettingsModal.tsx` 第 388 行附近（`{/* 终端跟随主题 */}` 那个 div 闭合之后、`<div className="text-base text-[var(--text-muted)] uppercase tracking-[0.1em] mb-2">字体大小</div>` 之前）插入：

```tsx
      {/* AI 完成弹框提醒 */}
      <div className="flex items-center justify-between px-3 py-2.5 rounded-[var(--radius-md)] bg-[var(--bg-base)] border border-[var(--border-subtle)] mb-3">
        <div>
          <div className="text-base text-[var(--text-primary)]">AI 完成弹框提醒</div>
          <div className="text-sm text-[var(--text-muted)]">AI 任务结束时在右下角弹出提醒卡片</div>
        </div>
        <button
          className={`relative w-9 h-5 rounded-full transition-colors ${
            config.aiCompletionPopup ? 'bg-[var(--accent)]' : 'bg-[var(--border-strong)]'
          }`}
          onClick={() => handleAiCompletionPopupChange(!config.aiCompletionPopup)}
        >
          <span
            className={`absolute top-0.5 left-0 w-4 h-4 rounded-full bg-white transition-transform ${
              config.aiCompletionPopup ? 'translate-x-[18px]' : 'translate-x-0.5'
            }`}
          />
        </button>
      </div>

      {/* AI 完成任务栏闪烁 */}
      <div className="flex items-center justify-between px-3 py-2.5 rounded-[var(--radius-md)] bg-[var(--bg-base)] border border-[var(--border-subtle)] mb-6">
        <div>
          <div className="text-base text-[var(--text-primary)]">AI 完成任务栏闪烁</div>
          <div className="text-sm text-[var(--text-muted)]">AI 任务结束且窗口失焦时闪烁任务栏图标（Windows 主要支持）</div>
        </div>
        <button
          className={`relative w-9 h-5 rounded-full transition-colors ${
            config.aiCompletionTaskbarFlash ? 'bg-[var(--accent)]' : 'bg-[var(--border-strong)]'
          }`}
          onClick={() => handleAiCompletionTaskbarFlashChange(!config.aiCompletionTaskbarFlash)}
        >
          <span
            className={`absolute top-0.5 left-0 w-4 h-4 rounded-full bg-white transition-transform ${
              config.aiCompletionTaskbarFlash ? 'translate-x-[18px]' : 'translate-x-0.5'
            }`}
          />
        </button>
      </div>
```

- [ ] **Step 3: 验证 TypeScript 编译**

Run: `cd D:/Git/mini-term && npx tsc --noEmit 2>&1`
Expected: 无新错误

- [ ] **Step 4: 提交**

```bash
cd D:/Git/mini-term && git add src/components/SettingsModal.tsx && git commit -m "$(cat <<'EOF'
feat: 设置面板新增 AI 完成提醒开关
- 系统设置页新增"AI 完成弹框提醒"toggle 控制右下角 toast 显示
- 系统设置页新增"AI 完成任务栏闪烁"toggle 控制 Windows 任务栏闪烁
- 两个开关默认开启，关闭后对应提醒方式立即停止生效
EOF
)"
```

---

### Task 8: 端到端手动验证

**Files:** 无代码改动

- [ ] **Step 1: 启动应用**

Run: `cd D:/Git/mini-term && npm run tauri dev`
Expected: 应用启动后无控制台报错

- [ ] **Step 2: 验证 Tag 显示和清除**

操作：
1. 准备至少 2 个项目（A、B）
2. 在项目 A 打开终端，运行 `claude` 进入 AI 会话
3. 切换到项目 B（让 A 变成非激活）
4. 等 A 的 AI 回应结束、3 秒后状态变为 ai-idle

预期：
- 项目 A 末尾出现绿色 "DONE" 胶囊（带微光晕）
- 项目 B 末尾不变化

5. 点击项目 A 切换激活
预期：DONE 胶囊消失，恢复为正常 StatusDot

- [ ] **Step 3: 验证 Toast 弹出和交互**

操作：重复 Step 2 的 1-4，观察右下角

预期：
- 右下角出现紧凑型 toast 卡片（绿色 ✓ 图标 + 项目名 + "AI 已完成 · 点击查看"）
- 卡片从右侧滑入
- 5 秒后自动消失

操作：再次触发，5 秒内点击 toast 卡片本体
预期：跳转到对应项目（设为激活），toast 消失

操作：再次触发，点击 toast 的 ×
预期：toast 消失，但项目的 DONE tag 仍在（因为没有激活该项目）

- [ ] **Step 4: 验证多 toast 堆叠**

操作：在多个非激活项目分别启动 AI，让它们同时完成

预期：右下角同时显示多个 toast，垂直堆叠，每个独立 5s 倒计时

- [ ] **Step 5: 验证激活项目不触发**

操作：在项目 A（保持激活状态）启动 AI 并等完成

预期：项目 A 末尾**不**出现 DONE tag，右下角**不**弹 toast

- [ ] **Step 6: 验证任务栏闪烁**

操作：在项目 A 启动 AI 后，立即按 Win+D 或切换到别的应用（让 mini-term 失焦）

预期：当 AI 完成时，任务栏的 mini-term 图标闪烁（橙色）

操作：点击任务栏 mini-term 图标恢复焦点
预期：闪烁停止

- [ ] **Step 7: 验证配置开关**

操作：打开"设置 → 系统设置"

预期：能看到"AI 完成弹框提醒"和"AI 完成任务栏闪烁"两个 toggle，默认开启

操作：关闭"AI 完成弹框提醒"toggle，再次触发非激活项目的 AI 完成

预期：DONE tag 仍出现（不可关），但 toast **不**弹出

操作：关闭"AI 完成任务栏闪烁"toggle，重复 Step 6 的操作

预期：任务栏**不**闪烁

- [ ] **Step 8: 验证持久化**

操作：关闭应用，重新启动

预期：上次设置的两个 toggle 状态被保留

- [ ] **Step 9: 验证启动恢复时不误触发**

操作：在某个项目跑完 AI（状态为 ai-idle），关闭应用，重新打开

预期：恢复时该项目**不**显示 DONE tag，**不**弹 toast，**不**闪任务栏（因为 oldStatus 不是 ai-working，不算 transition）

- [ ] **Step 10: 关闭 dev server，确认无问题**

完成上述所有验证后，按 Ctrl+C 停止 dev server。

如果发现任何问题，回到对应 Task 修复并补提交，然后重跑相关验证步骤。

---

## 实施完成检查

所有 8 个 Task 的勾选框都已勾选后，做最后一次全量检查：

```bash
cd D:/Git/mini-term && npm run build 2>&1
```

Expected: TypeScript 编译通过 + Vite 打包成功

```bash
cd D:/Git/mini-term/src-tauri && cargo build --release 2>&1
```

Expected: Rust 编译通过

如果两个都通过，则功能实现完整、可发布。
