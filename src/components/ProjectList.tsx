import { useCallback, useState } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { invoke } from '@tauri-apps/api/core';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { useAppStore, genId } from '../store';
import { StatusDot } from './StatusDot';
import { showContextMenu } from '../utils/contextMenu';
import type { PaneStatus, SplitNode } from '../types';

export function ProjectList() {
  const config = useAppStore((s) => s.config);
  const activeProjectId = useAppStore((s) => s.activeProjectId);
  const projectStates = useAppStore((s) => s.projectStates);
  const setActiveProject = useAppStore((s) => s.setActiveProject);
  const addProject = useAppStore((s) => s.addProject);
  const removeProject = useAppStore((s) => s.removeProject);

  const [confirmTarget, setConfirmTarget] = useState<{ id: string; name: string } | null>(null);

  const handleAddProject = useCallback(async () => {
    const selected = await open({ directory: true, multiple: false });
    if (!selected) return;

    const path = selected as string;
    const name = path.split(/[/\\]/).pop() || path;
    const id = genId();

    addProject({ id, name, path });

    // 持久化配置
    const latestConfig = useAppStore.getState().config;
    invoke('save_config', { config: latestConfig });
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
    const latestConfig = useAppStore.getState().config;
    invoke('save_config', { config: latestConfig });
    setConfirmTarget(null);
  }, [confirmTarget, removeProject]);

  // 获取项目的聚合状态（优先级：ai-idle > ai-working > idle）
  const getProjectStatus = (projectId: string): PaneStatus => {
    const ps = projectStates.get(projectId);
    if (!ps || ps.tabs.length === 0) return 'idle';

    const hasPaneWith = (node: SplitNode, target: PaneStatus): boolean => {
      if (node.type === 'leaf') return node.pane.status === target;
      return node.children.some((c) => hasPaneWith(c, target));
    };

    let hasAiWorking = false;
    for (const tab of ps.tabs) {
      if (hasPaneWith(tab.splitLayout, 'ai-idle')) return 'ai-idle';
      if (hasPaneWith(tab.splitLayout, 'ai-working')) hasAiWorking = true;
    }
    return hasAiWorking ? 'ai-working' : 'idle';
  };

  return (
    <div className="h-full bg-[var(--bg-surface)] flex flex-col overflow-y-auto">
      <div className="px-3 pt-3 pb-1.5 text-sm text-[var(--text-muted)] uppercase tracking-[0.12em] font-medium">
        Projects
      </div>

      <div className="flex-1 px-1.5 space-y-0.5">
        {config.projects.map((project) => {
          const isActive = project.id === activeProjectId;
          const projectStatus = getProjectStatus(project.id);

          return (
            <div
              key={project.id}
              className={`flex items-center gap-2 px-2.5 py-1.5 rounded-[var(--radius-sm)] cursor-pointer text-base group transition-all duration-150 ${
                isActive
                  ? 'bg-[var(--accent-subtle)] text-[var(--accent)]'
                  : 'text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--border-subtle)]'
              }`}
              onClick={() => setActiveProject(project.id)}
              onContextMenu={(e) => {
                e.preventDefault();
                e.stopPropagation();
                showContextMenu(e.clientX, e.clientY, [
                  {
                    label: '在文件夹中打开',
                    onClick: () => revealItemInDir(project.path),
                  },
                  {
                    label: '复制绝对路径',
                    onClick: () => navigator.clipboard.writeText(project.path),
                  },
                ]);
              }}
              title={project.path}
            >
              {isActive && (
                <span className="w-0.5 h-4 rounded-full bg-[var(--accent)] flex-shrink-0" />
              )}
              <span className="truncate flex-1">{project.name}</span>
              <StatusDot status={projectStatus} />
              <span
                className="text-[var(--text-muted)] hover:text-[var(--color-error)] hidden group-hover:inline transition-colors text-sm"
                onClick={(e) => handleRemoveProject(e, project.id)}
              >
                ✕
              </span>
            </div>
          );
        })}
      </div>

      <div className="p-2">
        <div
          className="px-3 py-2 border border-dashed border-[var(--border-default)] rounded-[var(--radius-md)] text-center text-sm text-[var(--text-muted)] cursor-pointer hover:border-[var(--accent)] hover:text-[var(--accent)] transition-all duration-200"
          onClick={handleAddProject}
        >
          + 添加项目
        </div>
      </div>

      {/* 删除确认弹窗 */}
      {confirmTarget && (
        <div className="fixed inset-0 z-50 flex items-center justify-center" onClick={() => setConfirmTarget(null)}>
          <div className="absolute inset-0 bg-black/50 backdrop-blur-sm" />
          <div
            className="relative w-[320px] bg-[var(--bg-surface)] border border-[var(--border-strong)] rounded-[var(--radius-md)] shadow-2xl p-5 animate-slide-in"
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
