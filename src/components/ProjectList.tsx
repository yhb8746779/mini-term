import { useCallback, useState, useRef, useEffect } from 'react';
import { Allotment } from 'allotment';
import { open } from '@tauri-apps/plugin-dialog';
import { invoke } from '@tauri-apps/api/core';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { useAppStore, genId } from '../store';
import { StatusDot } from './StatusDot';
import { DoneTag } from './DoneTag';
import { SessionList } from './SessionList';
import { showContextMenu } from '../utils/contextMenu';
import { showPrompt } from '../utils/prompt';
import { setDragPayload, getDragPayload } from '../utils/dragState';
import {
  getOrderedTree,
  collectAllGroups,
  countProjectsInGroup,
  canDrop,
  canDropAt,
  getDepth,
  findParentGroupId,
  findGroupInTree,
  MAX_DEPTH,
} from '../utils/projectTree';
import type { PaneStatus, AiProvider, PaneState, SplitNode, ProjectConfig, ProjectGroup } from '../types';

// 保存配置的快捷方法
function saveConfig() {
  const config = useAppStore.getState().config;
  invoke('save_config', { config });
}

// Drop 指示器位置
interface DropIndicator {
  id: string;
  position: 'before' | 'after' | 'inside';
  forbidden?: boolean;
}

export function ProjectList() {
  const config = useAppStore((s) => s.config);
  const activeProjectId = useAppStore((s) => s.activeProjectId);
  const projectStates = useAppStore((s) => s.projectStates);
  const setActiveProject = useAppStore((s) => s.setActiveProject);
  const addProject = useAppStore((s) => s.addProject);
  const removeProject = useAppStore((s) => s.removeProject);
  const createGroup = useAppStore((s) => s.createGroup);
  const removeGroup = useAppStore((s) => s.removeGroup);
  const renameGroup = useAppStore((s) => s.renameGroup);
  const toggleGroupCollapse = useAppStore((s) => s.toggleGroupCollapse);
  const moveItem = useAppStore((s) => s.moveItem);

  const [confirmTarget, setConfirmTarget] = useState<{ id: string; name: string } | null>(null);
  const [dropIndicator, setDropIndicator] = useState<DropIndicator | null>(null);
  const [editingGroupId, setEditingGroupId] = useState<string | null>(null);
  const [editingProjectId, setEditingProjectId] = useState<string | null>(null);
  const [editingName, setEditingName] = useState('');
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState('');
  const editInputRef = useRef<HTMLInputElement>(null);
  const editProjectInputRef = useRef<HTMLInputElement>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (searchOpen) searchInputRef.current?.focus();
  }, [searchOpen]);

  const closeSearch = useCallback(() => {
    setSearchOpen(false);
    setSearchQuery('');
  }, []);

  const filteredProjects = searchQuery.trim()
    ? config.projects.filter((p) => {
        const q = searchQuery.toLowerCase();
        return p.name.toLowerCase().includes(q) || p.path.toLowerCase().includes(q);
      })
    : null;

  const orderedItems = getOrderedTree(config);
  const allGroups = collectAllGroups(config.projectTree ?? []);

  const handleAddProject = useCallback(async () => {
    const selected = await open({ directory: true, multiple: false });
    if (!selected) return;
    const path = selected as string;
    const name = path.split(/[/\\]/).pop() || path;
    addProject({ id: genId(), name, path });
    saveConfig();
  }, [addProject]);

  const handleRemoveProject = useCallback(
    (e: React.MouseEvent, id: string) => {
      e.stopPropagation();
      const project = config.projects.find((p) => p.id === id);
      if (project) setConfirmTarget({ id, name: project.name });
    },
    [config.projects]
  );

  const doRemove = useCallback(() => {
    if (!confirmTarget) return;
    removeProject(confirmTarget.id);
    saveConfig();
    setConfirmTarget(null);
  }, [confirmTarget, removeProject]);

  // Provider 内各状态优先级
  const PROVIDER_STATUS_PRIORITY: Record<PaneStatus, number> = {
    error: 5, 'ai-awaiting-input': 4, 'ai-generating': 3, 'ai-thinking': 2, 'ai-complete': 1, idle: 0,
  };
  const PROVIDER_ORDER: AiProvider[] = ['claude', 'codex', 'gemini'];

  // 返回项目内各 provider 的最高优先级状态（固定顺序：claude > codex > gemini）
  const getProjectStatusSummary = (projectId: string): { provider: AiProvider; status: Exclude<PaneStatus, 'idle'> }[] => {
    const ps = projectStates.get(projectId);
    if (!ps || ps.tabs.length === 0) return [];

    const collectPanes = (node: SplitNode): PaneState[] => {
      if (node.type === 'leaf') return [...node.panes];
      return node.children.flatMap(collectPanes);
    };

    const allPanes = ps.tabs.flatMap((tab) => collectPanes(tab.splitLayout));
    const aiPanes = allPanes.filter((p) => p.status !== 'idle' && p.aiProvider);

    const result: { provider: AiProvider; status: Exclude<PaneStatus, 'idle'> }[] = [];
    for (const provider of PROVIDER_ORDER) {
      const panes = aiPanes.filter((p) => p.aiProvider === provider);
      if (panes.length === 0) continue;
      const best = panes.reduce<PaneStatus>(
        (acc, p) => PROVIDER_STATUS_PRIORITY[p.status] > PROVIDER_STATUS_PRIORITY[acc] ? p.status : acc,
        'idle',
      );
      if (best !== 'idle') {
        result.push({ provider, status: best as Exclude<PaneStatus, 'idle'> });
      }
    }

    // 如果没有带 provider 的 AI 状态，但有「有 provider 关联的 error」，也汇报
    // 不带 provider 的 error（普通终端退出非 0）不在此展示，避免误归属
    if (result.length === 0) {
      const providerErrors = allPanes.filter((p) => p.status === 'error' && p.aiProvider);
      for (const provider of PROVIDER_ORDER) {
        if (providerErrors.some((p) => p.aiProvider === provider)) {
          result.push({ provider, status: 'error' });
        }
      }
    }

    return result;
  };

  // 创建分组
  const handleCreateGroup = useCallback(async () => {
    const name = await showPrompt('新建分组', '请输入分组名称');
    if (!name?.trim()) return;
    createGroup(name.trim());
    saveConfig();
  }, [createGroup]);

  const renameProject = useAppStore((s) => s.renameProject);

  // 开始重命名项目
  const startRenameProject = useCallback((projectId: string, currentName: string) => {
    setEditingProjectId(projectId);
    setEditingName(currentName);
    setTimeout(() => editProjectInputRef.current?.select(), 0);
  }, []);

  // 提交项目重命名
  const commitProjectRename = useCallback(() => {
    if (editingProjectId && editingName.trim()) {
      renameProject(editingProjectId, editingName.trim());
      saveConfig();
    }
    setEditingProjectId(null);
  }, [editingProjectId, editingName, renameProject]);

  // 开始重命名分组
  const startRenameGroup = useCallback((groupId: string, currentName: string) => {
    setEditingGroupId(groupId);
    setEditingName(currentName);
    setTimeout(() => editInputRef.current?.select(), 0);
  }, []);

  // 提交重命名
  const commitRename = useCallback(() => {
    if (editingGroupId && editingName.trim()) {
      renameGroup(editingGroupId, editingName.trim());
      saveConfig();
    }
    setEditingGroupId(null);
  }, [editingGroupId, editingName, renameGroup]);

  // === 拖拽处理 ===

  const handleProjectDragStart = useCallback((e: React.DragEvent, projectId: string) => {
    e.dataTransfer.setData('application/project-id', projectId);
    e.dataTransfer.effectAllowed = 'move';
    setDragPayload({ type: 'project', projectId });
    // 添加拖拽时的半透明效果
    requestAnimationFrame(() => {
      (e.target as HTMLElement).style.opacity = '0.4';
    });
  }, []);

  const handleGroupDragStart = useCallback((e: React.DragEvent, groupId: string) => {
    e.dataTransfer.setData('application/group-id', groupId);
    e.dataTransfer.effectAllowed = 'move';
    setDragPayload({ type: 'group', groupId });
    requestAnimationFrame(() => {
      (e.target as HTMLElement).style.opacity = '0.4';
    });
  }, []);

  const handleDragEnd = useCallback((e: React.DragEvent) => {
    (e.target as HTMLElement).style.opacity = '';
    setDragPayload(null);
    setDropIndicator(null);
  }, []);

  const handleDragOver = useCallback((e: React.DragEvent, targetId: string, allowInside: boolean) => {
    const payload = getDragPayload();
    if (!payload) return;
    if (
      (payload.type === 'project' && payload.projectId === targetId) ||
      (payload.type === 'group' && payload.groupId === targetId)
    ) return;
    e.preventDefault();

    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const y = e.clientY - rect.top;
    const ratio = y / rect.height;

    let position: DropIndicator['position'];
    if (allowInside && ratio > 0.25 && ratio < 0.75) {
      position = 'inside';
    } else if (ratio < 0.5) {
      position = 'before';
    } else {
      position = 'after';
    }

    let forbidden = false;
    const tree = useAppStore.getState().config.projectTree ?? [];

    if (position === 'inside') {
      if (payload.type === 'project') {
        forbidden = !canDrop(tree, targetId, payload.projectId);
      } else {
        const draggedGroup = findGroupInTree(tree, payload.groupId);
        if (draggedGroup) {
          forbidden = !canDrop(tree, targetId, draggedGroup);
        }
      }
    } else if (payload.type === 'group') {
      const draggedGroup = findGroupInTree(tree, payload.groupId);
      if (draggedGroup) {
        forbidden = !canDropAt(tree, targetId, draggedGroup);
      }
    }

    e.dataTransfer.dropEffect = forbidden ? 'none' : 'move';
    setDropIndicator({ id: targetId, position, forbidden });
  }, []);

  const handleDragLeave = useCallback((e: React.DragEvent) => {
    const next = e.relatedTarget as Node | null;
    if (!next || !e.currentTarget.contains(next)) {
      setDropIndicator(null);
    }
  }, []);

  const handleDrop = useCallback((e: React.DragEvent, targetId: string) => {
    e.preventDefault();
    const payload = getDragPayload();
    if (!payload) return;
    const indicator = dropIndicator;
    setDropIndicator(null);
    setDragPayload(null);
    (e.target as HTMLElement).style.opacity = '';

    if (!indicator || indicator.forbidden) return;

    const itemId = payload.type === 'project' ? payload.projectId : payload.groupId;

    if (indicator.position === 'inside') {
      moveItem(itemId, targetId);
    } else {
      const tree = useAppStore.getState().config.projectTree ?? [];
      const parentGroupId = findParentGroupId(tree, targetId);
      const parent = parentGroupId
        ? findGroupInTree(tree, parentGroupId)?.children ?? []
        : tree;
      const targetIdx = parent.findIndex((item) =>
        (typeof item === 'string' ? item : item.id) === targetId
      );
      let insertIdx = indicator.position === 'after' ? targetIdx + 1 : targetIdx;
      const draggedIdx = parent.findIndex((item) =>
        (typeof item === 'string' ? item : item.id) === itemId
      );
      if (draggedIdx >= 0 && draggedIdx < insertIdx) {
        insertIdx--;
      }
      moveItem(itemId, parentGroupId ?? null, insertIdx);
    }
    saveConfig();
  }, [dropIndicator, moveItem]);

  // === 渲染子组件 ===

  const renderDropLine = (id: string, position: 'before' | 'after') => {
    if (dropIndicator?.id !== id || dropIndicator.position !== position) return null;
    if (dropIndicator.forbidden) return null;
    return (
      <div className="absolute left-1 right-1 h-0.5 bg-[var(--accent)] rounded-full z-10"
        style={position === 'before' ? { top: -1 } : { bottom: -1 }} />
    );
  };

  const renderProjectItem = (project: ProjectConfig, depth: number, parentGroupId?: string) => {
    const isActive = project.id === activeProjectId;
    const statusSummary = getProjectStatusSummary(project.id);
    const projectPs = projectStates.get(project.id);
    const showDoneTag = !!projectPs?.needsAttention && !isActive;

    return (
      <div
        key={project.id}
        className={`relative flex items-center gap-2 py-1.5 rounded-[var(--radius-sm)] cursor-pointer text-base group transition-all duration-150 ${
          isActive
            ? 'bg-[var(--accent-subtle)] text-[var(--accent)]'
            : 'text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--border-subtle)]'
        }`}
        style={{ paddingLeft: `${depth * 16 + 10}px`, paddingRight: '10px' }}
        draggable
        onDragStart={(e) => handleProjectDragStart(e, project.id)}
        onDragEnd={handleDragEnd}
        onDragOver={(e) => handleDragOver(e, project.id, false)}
        onDragLeave={handleDragLeave}
        onDrop={(e) => handleDrop(e, project.id)}
        onClick={() => setActiveProject(project.id)}
        onContextMenu={(e) => {
          e.preventDefault();
          e.stopPropagation();
          const menuItems: Parameters<typeof showContextMenu>[2] = [
            { label: '重命名', onClick: () => startRenameProject(project.id, project.name) },
            { label: '在文件夹中打开', onClick: () => revealItemInDir(project.path) },
            { label: '复制绝对路径', onClick: () => navigator.clipboard.writeText(project.path) },
          ];
          // 添加分组相关菜单
          if (allGroups.length > 0) {
            menuItems.push({ separator: true });
            if (parentGroupId) {
              menuItems.push({
                label: '移出分组',
                onClick: () => { moveItem(project.id, null); saveConfig(); },
              });
            }
            for (const [g, gDepth] of allGroups) {
              if (g.id === parentGroupId) continue;
              if (gDepth + 1 > MAX_DEPTH) continue;
              menuItems.push({
                label: `移动到「${g.name}」`,
                onClick: () => { moveItem(project.id, g.id); saveConfig(); },
              });
            }
          }
          showContextMenu(e.clientX, e.clientY, menuItems);
        }}
        title={project.path}
      >
        {renderDropLine(project.id, 'before')}
        {isActive && (
          <span className="w-0.5 h-4 rounded-full bg-[var(--accent)] flex-shrink-0" />
        )}
        {editingProjectId === project.id ? (
          <input
            ref={editProjectInputRef}
            className="truncate flex-1 bg-transparent border-b border-[var(--accent)] outline-none text-base text-[var(--text-primary)] px-0 py-0"
            value={editingName}
            onChange={(e) => setEditingName(e.target.value)}
            onBlur={commitProjectRename}
            onKeyDown={(e) => {
              if (e.key === 'Enter') commitProjectRename();
              if (e.key === 'Escape') setEditingProjectId(null);
            }}
            onClick={(e) => e.stopPropagation()}
            autoFocus
          />
        ) : (
          <span className="truncate flex-1">{project.name}</span>
        )}
        {/* AI 完成提醒优先显示；否则 provider 状态汇总（每个 provider 一个点，固定顺序） */}
        {showDoneTag ? <DoneTag /> : (
          <div className="flex items-center gap-0.5 flex-shrink-0">
            {statusSummary.length === 0 ? (
              <StatusDot status="idle" />
            ) : (
              statusSummary.map(({ provider, status }) => (
                <StatusDot key={provider} status={status} provider={provider} />
              ))
            )}
          </div>
        )}
        <span
          className="text-[var(--text-muted)] hover:text-[var(--color-error)] hidden group-hover:inline transition-colors text-sm"
          onClick={(e) => handleRemoveProject(e, project.id)}
        >
          ✕
        </span>
        {renderDropLine(project.id, 'after')}
      </div>
    );
  };

  const renderGroup = (group: ProjectGroup, depth: number) => {
    const isEditing = editingGroupId === group.id;
    const isInsideTarget = dropIndicator?.id === group.id && dropIndicator.position === 'inside';
    const isForbidden = isInsideTarget && dropIndicator?.forbidden;
    const groupDepth = getDepth(config.projectTree ?? [], group.id);

    return (
      <div key={group.id} className="relative">
        {renderDropLine(group.id, 'before')}
        <div
          className={`flex items-center gap-1.5 py-1.5 rounded-[var(--radius-sm)] cursor-pointer text-sm transition-all duration-150 select-none ${
            isForbidden
              ? 'border border-dashed border-[var(--color-error)] cursor-not-allowed'
              : isInsideTarget
                ? 'bg-[var(--accent-subtle)] border border-dashed border-[var(--accent)]'
                : 'text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--border-subtle)]'
          }`}
          style={{ paddingLeft: `${depth * 16}px`, paddingRight: '10px' }}
          draggable={!isEditing}
          onDragStart={(e) => handleGroupDragStart(e, group.id)}
          onDragEnd={handleDragEnd}
          onDragOver={(e) => handleDragOver(e, group.id, true)}
          onDragLeave={handleDragLeave}
          onDrop={(e) => handleDrop(e, group.id)}
          onClick={() => { if (!isEditing) toggleGroupCollapse(group.id); saveConfig(); }}
          onContextMenu={(e) => {
            e.preventDefault();
            e.stopPropagation();
            const menuItems: Parameters<typeof showContextMenu>[2] = [
              { label: '重命名分组', onClick: () => startRenameGroup(group.id, group.name) },
            ];
            if (depth > 0) {
              menuItems.push({
                label: '移出分组',
                onClick: () => { moveItem(group.id, null); saveConfig(); },
              });
            }
            if (groupDepth < MAX_DEPTH - 1) {
              menuItems.push({
                label: '新建子组',
                onClick: async () => {
                  const name = await showPrompt('新建子组', '请输入子组名称');
                  if (!name?.trim()) return;
                  createGroup(name.trim(), group.id);
                  saveConfig();
                },
              });
            }
            menuItems.push(
              { label: '删除分组（保留项目）', danger: true, onClick: () => { removeGroup(group.id); saveConfig(); } },
            );
            showContextMenu(e.clientX, e.clientY, menuItems);
          }}
        >
          <span className="text-xs flex-shrink-0 w-3 text-center transition-transform duration-150"
            style={{ transform: group.collapsed ? 'rotate(-90deg)' : undefined }}>
            ▾
          </span>
          {isEditing ? (
            <input
              ref={editInputRef}
              className="flex-1 bg-transparent border-b border-[var(--accent)] outline-none text-sm text-[var(--text-primary)] px-0 py-0"
              value={editingName}
              onChange={(e) => setEditingName(e.target.value)}
              onBlur={commitRename}
              onKeyDown={(e) => {
                if (e.key === 'Enter') commitRename();
                if (e.key === 'Escape') setEditingGroupId(null);
              }}
              onClick={(e) => e.stopPropagation()}
              autoFocus
            />
          ) : (
            <span className="truncate flex-1 font-medium">{group.name}</span>
          )}
          <span className="text-xs text-[var(--text-muted)]">({countProjectsInGroup(group)})</span>
        </div>
        {renderDropLine(group.id, 'after')}
      </div>
    );
  };

  return (
    <div className="h-full bg-[var(--bg-surface)] flex flex-col">
      <Allotment vertical>
        {/* 上半部分：项目列表 */}
        <Allotment.Pane minSize={100}>
          <div className="h-full flex flex-col overflow-hidden">
            <div
              className="px-3 pt-3 pb-1.5 flex items-center gap-1"
              onContextMenu={(e) => {
                e.preventDefault();
                showContextMenu(e.clientX, e.clientY, [
                  { label: '新建分组', onClick: handleCreateGroup },
                ]);
              }}
            >
              <span className="flex-1 text-sm text-[var(--text-muted)] uppercase tracking-[0.12em] font-medium cursor-default">
                Projects
              </span>
              <button
                className={`w-5 h-5 flex items-center justify-center rounded transition-colors ${
                  searchOpen
                    ? 'text-[var(--accent)] bg-[var(--accent-subtle)]'
                    : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
                }`}
                title="搜索项目 (Ctrl+F)"
                onClick={() => (searchOpen ? closeSearch() : setSearchOpen(true))}
              >
                <svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round">
                  <circle cx="5" cy="5" r="3.5" />
                  <line x1="7.8" y1="7.8" x2="11" y2="11" />
                </svg>
              </button>
            </div>

            {searchOpen && (
              <div className="px-2 pb-1.5">
                <div className="relative">
                  <input
                    ref={searchInputRef}
                    className="w-full bg-[var(--bg-elevated)] border border-[var(--border-default)] focus:border-[var(--accent)] rounded-[var(--radius-sm)] px-2.5 py-1 text-xs text-[var(--text-primary)] outline-none placeholder:text-[var(--text-muted)] pr-6 transition-colors"
                    placeholder="搜索项目名称或路径…"
                    value={searchQuery}
                    onChange={(e) => setSearchQuery(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Escape') closeSearch();
                    }}
                  />
                  {searchQuery && (
                    <button
                      className="absolute right-1.5 top-1/2 -translate-y-1/2 text-[var(--text-muted)] hover:text-[var(--text-primary)] transition-colors"
                      onClick={() => { setSearchQuery(''); searchInputRef.current?.focus(); }}
                    >
                      <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
                        <line x1="1.5" y1="1.5" x2="8.5" y2="8.5" />
                        <line x1="8.5" y1="1.5" x2="1.5" y2="8.5" />
                      </svg>
                    </button>
                  )}
                </div>
              </div>
            )}

            <div className="flex-1 overflow-y-auto px-1.5 space-y-0.5">
              {filteredProjects ? (
                filteredProjects.length === 0 ? (
                  <div className="px-2 py-3 text-xs text-[var(--text-muted)] text-center">无匹配项目</div>
                ) : (
                  filteredProjects.map((project) => (
                    <div key={project.id}>
                      {renderProjectItem(project, 0)}
                      {searchQuery.trim() && (
                        <div className="px-2.5 pb-0.5 text-[10px] text-[var(--text-muted)] truncate leading-tight -mt-0.5">
                          {project.path}
                        </div>
                      )}
                    </div>
                  ))
                )
              ) : (
                orderedItems.map((item) =>
                  item.type === 'project'
                    ? renderProjectItem(item.project, item.depth, item.parentGroupId ?? undefined)
                    : renderGroup(item.group, item.depth)
                )
              )}
            </div>

            <div className="p-2 flex gap-1.5">
              <div
                className="flex-1 px-3 py-2 border border-dashed border-[var(--border-default)] rounded-[var(--radius-md)] text-center text-sm text-[var(--text-muted)] cursor-pointer hover:border-[var(--accent)] hover:text-[var(--accent)] transition-all duration-200"
                onClick={handleAddProject}
              >
                + 添加项目
              </div>
              <div
                className="px-3 py-2 border border-dashed border-[var(--border-default)] rounded-[var(--radius-md)] text-center text-sm text-[var(--text-muted)] cursor-pointer hover:border-[var(--accent)] hover:text-[var(--accent)] transition-all duration-200"
                onClick={handleCreateGroup}
                title="新建分组"
              >
                +
              </div>
            </div>
          </div>
        </Allotment.Pane>

        {/* 下半部分：会话列表 */}
        <Allotment.Pane minSize={80}>
          <SessionList />
        </Allotment.Pane>
      </Allotment>

      {/* 删除确认弹窗 */}
      {confirmTarget && (
        <div className="fixed inset-0 z-50 flex items-center justify-center" onClick={() => setConfirmTarget(null)}>
          <div className="absolute inset-0 bg-black/50 backdrop-blur-sm" />
          <div
            className="relative w-[320px] bg-[var(--bg-surface)] border border-[var(--border-strong)] rounded-[var(--radius-md)] shadow-[var(--shadow-overlay)] p-5 animate-slide-in"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="text-sm font-medium text-[var(--text-primary)] mb-2">移除项目</div>
            <div className="text-xs text-[var(--text-secondary)] mb-5">
              确定要移除项目「<span className="text-[var(--accent)]">{confirmTarget.name}</span>」吗？此操作仅从列表中移除，不会删除文件。
            </div>
            <div className="flex justify-end gap-2">
              <button
                className="px-3 py-1.5 text-xs rounded-[var(--radius-sm)] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--border-subtle)] transition-colors"
                onClick={() => setConfirmTarget(null)}
              >
                取消
              </button>
              <button
                className="px-3 py-1.5 text-xs rounded-[var(--radius-sm)] bg-[var(--color-error)] text-white hover:opacity-90 transition-opacity"
                onClick={doRemove}
              >
                移除
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
