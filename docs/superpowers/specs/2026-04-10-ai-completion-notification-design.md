# AI 完成提醒设计

## 概述

当 Claude/Codex 在某个项目中执行结束（pty 状态从 `ai-working` → `ai-idle`），触发三层提醒：

1. **Tag** — 该项目在项目列表末尾出现一个绿色 "DONE" 胶囊标记（始终启用）
2. **Toast** — 页面右下角弹出紧凑型卡片，5 秒自动消失（可配置开关）
3. **任务栏闪烁** — 调用 `requestUserAttention(Informational)`，Windows 任务栏图标闪烁直到用户聚焦窗口（可配置开关）

Tag 在用户激活该项目时清除。当前正在浏览的项目不触发 Tag/Toast（用户已在场），但任务栏闪烁始终触发——Tauri 的 `requestUserAttention` API 自带"已聚焦时无效果"判断，所以不需要前端管理 focus 状态。

解决的问题：现有的 `StatusDot` 颜色变化（`ai-working` 黄 → `ai-idle` 绿）太弱，用户经常错过 AI 任务完成的时机。

## 核心设计

**三个独立概念，不要混在一起：**

| | Tag (DONE 胶囊) | Toast (右下弹框) | 任务栏闪烁 |
|---|---|---|---|
| 性质 | 持久状态 | 临时事件 | 一次性副作用 |
| 存储 | `ProjectState.needsAttention: boolean` | `notifications: AiCompletionNotification[]` (store 切片) | 无（直接调 Tauri API） |
| 触发 | pty `ai-working` → `ai-idle` 且非激活项目 | 同 Tag + 配置开关为 ON | pty `ai-working` → `ai-idle` + 配置开关为 ON（不区分激活项目） |
| 聚焦时行为 | 正常（如果不是该项目激活） | 正常（如果不是该项目激活） | Tauri API 自动忽略 |
| 清除 | `setActiveProject(pid)` | 5s 倒计时 / × / 点击跳转 | 用户聚焦窗口时 OS 自动解除 |
| 可配置 | 否 | 是（`aiCompletionPopup` 默认 `true`） | 是（`aiCompletionTaskbarFlash` 默认 `true`） |

## 数据模型

### `src/types.ts`

```typescript
// 扩展 ProjectState
export interface ProjectState {
  id: string;
  tabs: TerminalTab[];
  activeTabId: string;
  needsAttention?: boolean;  // 新增
}

// 新增 AiCompletionNotification 类型
export interface AiCompletionNotification {
  id: string;
  projectId: string;
  projectName: string;
  timestamp: number;
}

// 扩展 AppConfig
export interface AppConfig {
  // ... 现有字段
  aiCompletionPopup: boolean;        // 新增 — 控制右下角 toast
  aiCompletionTaskbarFlash: boolean; // 新增 — 控制 Windows 任务栏闪烁
}
```

### `src-tauri/src/config.rs`

`AppConfig` struct (行 34-56) 加：

```rust
#[serde(default = "default_ai_completion_popup")]
pub ai_completion_popup: bool,

#[serde(default = "default_ai_completion_taskbar_flash")]
pub ai_completion_taskbar_flash: bool,
```

新增两个 helper 函数（与现有 `default_terminal_follow_theme` 风格一致）：

```rust
fn default_ai_completion_popup() -> bool { true }
fn default_ai_completion_taskbar_flash() -> bool { true }
```

`Default` 实现 (行 130-147) 中调用这两个函数初始化字段。

## 前端实现

### `src/store.ts`

**顶层 import：** 在文件头部新增 `import { getCurrentWindow, UserAttentionType } from '@tauri-apps/api/window';`（该模块已在 `App.tsx:5` 静态使用，所以再加一处不会增加 bundle 体积）。

**新增 store 切片：**

```typescript
interface AppStore {
  // ... 现有
  notifications: AiCompletionNotification[];
  pushNotification: (n: Omit<AiCompletionNotification, 'id' | 'timestamp'>) => void;
  dismissNotification: (id: string) => void;
}
```

`pushNotification` 实现内部使用 `genId()`（已从 store.ts 顶层导出）生成 `id`，`Date.now()` 生成 `timestamp`，再 push 到 `state.notifications`。

**修改 `updatePaneStatusByPty` (行 436-464)：**

关键约束：必须在调用 `updatePaneStatus` **之前**捕获 oldStatus，否则更新后无法判断 transition。实施时遍历所有 pane 收集 `(ptyId, oldStatus)` 映射，再做更新。

```typescript
updatePaneStatusByPty: (ptyId, status) =>
  set((state) => {
    // 1. 捕获 oldStatus
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

    // 2. 现有的状态更新逻辑（保持不变）
    const newStates = /* 同现有逻辑 */;

    // 3. 检测 transition：ai-working → ai-idle
    const isCompletion = oldStatus === 'ai-working' && status === 'ai-idle';
    if (isCompletion) {
      // 3a. 任务栏闪烁 — 不区分激活项目，因为 Tauri API 自带 focus 检测
      if (state.config.aiCompletionTaskbarFlash) {
        // getCurrentWindow / UserAttentionType 已在 store.ts 顶层 import
        queueMicrotask(() => {
          getCurrentWindow().requestUserAttention(UserAttentionType.Informational).catch(() => {});
        });
      }

      // 3b. Tag + Toast — 仅非激活项目
      if (owningProjectId !== state.activeProjectId) {
        const ps = newStates.get(owningProjectId);
        if (ps && !ps.needsAttention) {
          // 防重：同项目已 needsAttention 则不重复
          newStates.set(owningProjectId, { ...ps, needsAttention: true });

          // 推 Toast（同项目当前没有未消失的 toast 才推）
          if (state.config.aiCompletionPopup) {
            const project = state.config.projects.find((p) => p.id === owningProjectId);
            const hasExisting = state.notifications.some((n) => n.projectId === owningProjectId);
            if (project && !hasExisting) {
              // 通过 microtask 触发 push（避免在 set 期间嵌套 set）
              queueMicrotask(() => useAppStore.getState().pushNotification({
                projectId: owningProjectId!,
                projectName: project.name,
              }));
            }
          }
        }
      }
    }

    return { projectStates: newStates };
  }),
```

**为什么任务栏闪烁不需要激活项目检查：** Tauri 的 `requestUserAttention` 文档明确：*"This has no effect if the application is already focused."* 所以即便用户正在看着应用，调用也是 no-op；只有在窗口失焦（用户切到别的应用）时才会真正闪。这正好对应"用户当前没在看 mini-term"的语义。

**修改 `setActiveProject` (行 336)：**

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

**修改 `removeProject` (行 356)：**

清理该 pid 的所有 notifications：

```typescript
removeProject: (id) =>
  set((state) => {
    // ... 现有逻辑
    return {
      // ...
      notifications: state.notifications.filter((n) => n.projectId !== id),
    };
  }),
```

**新增辅助函数：**

```typescript
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

### `src/components/DoneTag.tsx` (新建)

```tsx
export function DoneTag() {
  return <span className="done-tag">DONE</span>;
}
```

### `src/components/ProjectList.tsx`

`renderProjectItem` (行 270-351) 中，在 `<StatusDot>` (行 341) 位置改为：

```tsx
const ps = projectStates.get(project.id);
const showDoneTag = ps?.needsAttention && !isActive;

{showDoneTag
  ? <DoneTag />
  : <StatusDot status={projectStatus} />
}
```

### `src/components/ToastContainer.tsx` (新建)

```tsx
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

  // 最多同时渲染 5 个，超出排队（按 timestamp 排序后取前 5）
  const visible = notifications.slice(0, 5);

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

挂载到 `App.tsx` 顶层（在 Allotment 主布局之外，作为 fixed 定位的覆盖层）。

### `src/components/SettingsModal.tsx`

在 `SystemSettings` 组件中加两行 toggle，照搬 `terminalFollowTheme` (行 332-385) 的样式：

```tsx
<SettingRow label="AI 完成弹框提醒" desc="AI 任务结束时在右下角弹出提醒卡片">
  <Toggle
    checked={config.aiCompletionPopup}
    onChange={(v) => {
      const newConfig = { ...config, aiCompletionPopup: v };
      setConfig(newConfig);
      invoke('save_config', { config: newConfig });
    }}
  />
</SettingRow>

<SettingRow label="AI 完成任务栏闪烁" desc="AI 任务结束且窗口失焦时，闪烁任务栏图标提醒（Windows 主要支持）">
  <Toggle
    checked={config.aiCompletionTaskbarFlash}
    onChange={(v) => {
      const newConfig = { ...config, aiCompletionTaskbarFlash: v };
      setConfig(newConfig);
      invoke('save_config', { config: newConfig });
    }}
  />
</SettingRow>
```

## 样式

### `src/styles.css`

```css
/* === DONE Tag === */
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

/* === Toast === */
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

## 行为细节

### Tag
- **出现**：state 设置后立刻渲染，带 0.3s `tagFadeIn` 动画
- **静止状态**：保持原样，无持续动画（避免视觉疲劳）
- **消失**：`setActiveProject` 触发时直接消失，无动画
- **替换关系**：Tag 出现时**完全替换** `<StatusDot>`，不并排

### Toast
- **入场**：从右侧滑入，0.25s `toastSlideIn`
- **生命周期**：5s 后自动 dismiss
- **点击区域**：整张卡片（除 × 按钮）→ `setActiveProject(projectId)` + dismiss 该 toast
- **× 按钮**：仅 dismiss 该 toast，**不**清除目标项目的 needsAttention
- **堆叠**：最多渲染 5 个，超出排队等前面消失

### 任务栏闪烁
- **触发**：每次 transition 都调一次 `requestUserAttention(Informational)`
- **聚焦时**：Tauri API 自动判定为 no-op，不影响用户
- **失焦时**：Windows 任务栏图标闪烁（橙色），直到用户点击窗口
- **解除**：用户聚焦窗口时 OS 自动停止闪烁，**不需要前端调用** `requestUserAttention(null)`
- **重复触发**：失焦期间多次完成会重复调用，但视觉效果仅是"持续闪烁"，无副作用
- **跨平台行为**：macOS 会变成 Dock 图标弹一下；Linux 取决于 WM。本设计以 Windows 为主要目标但保持跨平台兼容

## 边界情况

| 场景 | 行为 |
|---|---|
| 同项目内多个 pane 同时 working→idle | 只在首次触发时设 needsAttention 和推 toast，后续 pane 静默（防重） |
| 同项目 AI 完成后再次启动→再次完成 | 若 needsAttention 已为 true 则不重复设置；toast 同项目已存在则不重复推（防刷屏） |
| 项目被删除（`removeProject`） | 同步清理 `notifications` 中该 pid 的所有项 |
| Pane 退出/出错（`error` 状态） | 不视作完成，不触发 |
| 应用启动恢复时 pane 状态已是 ai-idle | 不触发，因为 oldStatus 不是 ai-working（只有运行时 transition 算） |
| 用户在浏览 A，B 完成 → 切到 B → C 完成 | Tag: B 自动清，C 出现；Toast: B、C 各自独立 5s 倒计时 |
| Toast 显示中用户点击该项目 | Toast 仍在,但 Tag 已清(因为 setActiveProject 触发)。Toast 自然到时消失 |

## 不实现 (YAGNI)

- 系统级桌面通知（Tauri notification plugin）
- 提示音
- 配置 toast 持续时长（写死 5s）
- "标记全部已读"批量操作
- 配置 toast 排队上限（写死 5）
- Tag 的开关配置（始终启用）
- 闪烁强度可选（写死 `Informational`，不用 `Critical` 因为后者会同时闪窗口边框，过于侵入）

## 实施顺序

1. **数据层** — `types.ts` + `config.rs` 加 `aiCompletionPopup` 和 `aiCompletionTaskbarFlash` 字段；`store.ts` 加 `notifications` 切片和 `pushNotification`/`dismissNotification`
2. **检测层** — `updatePaneStatusByPty` 加 transition 逻辑（含 Tag、Toast、TaskbarFlash 三个分支）；`setActiveProject` 加清除逻辑；`removeProject` 加清理
3. **Tag 视觉** — `DoneTag.tsx` + `.done-tag` CSS；`ProjectList.tsx` 替换 StatusDot 渲染
4. **Toast 视觉** — `ToastContainer.tsx` + `.toast-*` CSS；挂载到 `App.tsx`
5. **任务栏闪烁** — 在检测层中调用 `requestUserAttention(Informational)`（无独立组件，仅一次副作用）
6. **配置 UI** — `SettingsModal.tsx` 加两个 toggle 行
7. **手动验证** — `npm run tauri dev`，跑 `claude` 命令，等结束观察：
   - Tag 出现在非激活项目，激活后清除
   - Toast 弹出，5s 自动消失，× 关闭，点击跳转
   - 切到别的应用 → AI 完成 → 任务栏闪烁 → 点回 mini-term 停止
   - 关闭 popup config → toast 不弹但 Tag 和 闪烁仍生效
   - 关闭 taskbar flash config → 闪烁不触发
   - 多个项目同时完成 → toast 堆叠

## 风险点

- **Transition 检测准确性**：`updatePaneStatusByPty` 必须在更新 pane 状态**之前**捕获 oldStatus，否则 transition 永远检测不到。这是最容易出错的点
- **Map 浅拷贝**：`needsAttention` 变更必须创建新的 `ProjectState` 对象（`{ ...ps, needsAttention: true }`），否则 React 不会重渲染 `ProjectList`
- **Toast 卸载**：`useEffect` 中的 `setTimeout` 必须在依赖变化和 unmount 时清理，否则会在已 dismiss 的 toast 上重复调用
- **防重逻辑的微妙性**：toast 防重用 `notifications.some(...)` 检查，意味着同项目快速完成两次只显示一次。这是有意的（防刷屏），但用户若需要每次都通知则要重新讨论
- **Linux 兼容性**：`requestUserAttention` 在 Linux 上的行为依赖窗口管理器（GNOME/KDE 等），可能完全无效或表现不一致。配置开关默认开启可能会让 Linux 用户疑惑"为什么没反应"——但因主要目标是 Windows，且 API 调用本身是 no-op 不会出错，已在 Settings UI 的描述中标注"Windows 主要支持"
