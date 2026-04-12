import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { message } from '@tauri-apps/plugin-dialog';
import { revealItemInDir, openPath } from '@tauri-apps/plugin-opener';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import { useAppStore, isExpanded, toggleExpandedDir } from '../store';
import { useTauriEvent } from '../hooks/useTauriEvent';
import { showContextMenu } from '../utils/contextMenu';
import { showPrompt } from '../utils/prompt';
import { DiffModal } from './DiffModal';
import { FileViewerModal } from './FileViewerModal';
import type { FileEntry, FsChangePayload, GitFileStatus, PtyOutputPayload } from '../types';

interface TreeNodeProps {
  entry: FileEntry;
  projectRoot: string;
  depth: number;
  gitStatusMap: Map<string, GitFileStatus>;
  onViewDiff: (status: GitFileStatus) => void;
  onViewFile: (path: string) => void;
}

function getRelativePath(targetPath: string, rootPath: string) {
  const normalize = (value: string) => value.replace(/[\\/]+/g, '/').replace(/\/$/, '');
  const normalizedRoot = normalize(rootPath);
  const normalizedTarget = normalize(targetPath);
  const sep = rootPath.includes('\\') ? '\\' : '/';

  if (normalizedTarget === normalizedRoot) return '.';
  if (!normalizedTarget.startsWith(`${normalizedRoot}/`)) return targetPath;

  return normalizedTarget.slice(normalizedRoot.length + 1).replace(/\//g, sep);
}

function TreeNode({ entry, projectRoot, depth, gitStatusMap, onViewDiff, onViewFile }: TreeNodeProps) {
  const activeProjectId = useAppStore((s) => s.activeProjectId);
  const [expanded, setExpanded] = useState(() =>
    activeProjectId ? isExpanded(activeProjectId, entry.path) : false
  );
  const [children, setChildren] = useState<FileEntry[]>([]);

  const loadChildren = useCallback(async () => {
    const entries = await invoke<FileEntry[]>('list_directory', {
      projectRoot,
      path: entry.path,
    });
    setChildren(entries);
  }, [entry.path, projectRoot]);

  // 恢复时自动加载子节点并注册监听
  useEffect(() => {
    if (expanded && entry.isDir) {
      loadChildren();
      invoke('watch_directory', { path: entry.path, projectPath: projectRoot });
    }
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const handleToggle = useCallback(async () => {
    if (!entry.isDir) {
      const rel = getRelativePath(entry.path, projectRoot).replace(/\\/g, '/');
      const fileStatus = gitStatusMap.get(rel);
      if (fileStatus) {
        onViewDiff(fileStatus);
      } else {
        onViewFile(entry.path);
      }
      return;
    }
    const next = !expanded;
    if (next) {
      await loadChildren();
      invoke('watch_directory', { path: entry.path, projectPath: projectRoot });
    } else {
      invoke('unwatch_directory', { path: entry.path });
    }
    setExpanded(next);
    if (activeProjectId) {
      toggleExpandedDir(activeProjectId, entry.path, next);
    }
  }, [entry, expanded, loadChildren, projectRoot, gitStatusMap, onViewDiff, onViewFile, activeProjectId]);

  useTauriEvent<FsChangePayload>('fs-change', useCallback((payload: FsChangePayload) => {
    if (expanded && payload.path.startsWith(entry.path)) {
      loadChildren();
    }
  }, [expanded, entry.path, loadChildren]));

  return (
    <div>
      <div
        className={`flex items-center gap-1 py-[3px] cursor-pointer hover:bg-[var(--border-subtle)] rounded-[var(--radius-sm)] text-base transition-colors duration-100 ${
          entry.ignored ? 'text-[var(--text-muted)] opacity-50' : entry.isDir ? 'text-[var(--color-folder)]' : 'text-[var(--color-file)]'
        }`}
        style={{ paddingLeft: `${depth * 16 + 8}px` }}
        onClick={handleToggle}
        onContextMenu={(e) => {
          e.preventDefault();
          e.stopPropagation();
          const relativePath = getRelativePath(entry.path, projectRoot);
          const items: Parameters<typeof showContextMenu>[2] = [
            {
              label: '复制相对路径',
              onClick: () => writeText(relativePath),
            },
            {
              label: '复制绝对路径',
              onClick: () => writeText(entry.path),
            },
            { separator: true },
            {
              label: '在文件夹中打开',
              onClick: () => revealItemInDir(entry.path),
            },
          ];
          if (!entry.isDir) {
            items.unshift({
              label: '使用默认工具打开',
              onClick: () => openPath(entry.path),
            });
          }
          items.push({ separator: true });
          items.push({
            label: '重命名',
            onClick: async () => {
              const newName = await showPrompt('重命名', '请输入新名称', entry.name);
              if (!newName?.trim() || newName.trim() === entry.name) return;
              try {
                await invoke('rename_entry', { oldPath: entry.path, newName: newName.trim() });
                loadChildren();
              } catch (err) {
                console.error('重命名失败:', err);
              }
            },
          });
          if (entry.isDir) {
            items.push({ separator: true });
            items.push({
              label: '新建文件',
              onClick: async () => {
                const name = await showPrompt('新建文件', '请输入文件名');
                if (!name?.trim()) return;
                const sep = entry.path.includes('/') ? '/' : '\\';
                await invoke('create_file', { path: `${entry.path}${sep}${name.trim()}` });
                if (!expanded) handleToggle();
                else loadChildren();
              },
            });
            items.push({
              label: '新建文件夹',
              onClick: async () => {
                const name = await showPrompt('新建文件夹', '请输入文件夹名');
                if (!name?.trim()) return;
                const sep = entry.path.includes('/') ? '/' : '\\';
                await invoke('create_directory', { path: `${entry.path}${sep}${name.trim()}` });
                if (!expanded) handleToggle();
                else loadChildren();
              },
            });
          }
          // 查看变更菜单项
          const relForGit = getRelativePath(entry.path, projectRoot).replace(/\\/g, '/');
          const entryGitStatus = gitStatusMap.get(relForGit);
          if (entryGitStatus && !entry.isDir) {
            items.push({ separator: true });
            items.push({
              label: '查看变更',
              onClick: () => onViewDiff(entryGitStatus),
            });
          }
          showContextMenu(e.clientX, e.clientY, items);
        }}
        draggable
        onDragStart={(e) => {
          e.dataTransfer.setData('text/plain', entry.path);
          e.dataTransfer.effectAllowed = 'copy';
        }}
      >
        {entry.isDir && (
          <span className="text-[13px] w-3 text-center text-[var(--text-muted)] transition-transform duration-150"
            style={{ transform: expanded ? 'rotate(0deg)' : 'rotate(-90deg)', display: 'inline-block' }}>
            ▾
          </span>
        )}
        {!entry.isDir && <span className="w-3 text-center text-[var(--text-muted)] text-xs">·</span>}
        <span className="truncate" title={entry.name}>{entry.name}</span>
        {(() => {
          const rel = getRelativePath(entry.path, projectRoot).replace(/\\/g, '/');
          const fileStatus = gitStatusMap.get(rel);
          const GIT_COLORS: Record<string, string> = {
            M: 'text-[var(--color-warning)]',
            A: 'text-[var(--color-success)]',
            D: 'text-[var(--color-error)]',
            R: 'text-[var(--color-info)]',
            '?': 'text-[var(--color-success)]',
            C: 'text-[var(--color-error)]',
          };
          if (fileStatus) {
            return (
              <span className={`ml-1.5 text-xs font-bold flex-shrink-0 ${GIT_COLORS[fileStatus.statusLabel] ?? 'text-[var(--text-muted)]'}`}>
                {fileStatus.statusLabel}
              </span>
            );
          }
          if (entry.isDir) {
            const prefix = rel.endsWith('/') ? rel : rel + '/';
            const PRIORITY: Record<string, number> = { C: 6, D: 5, M: 4, A: 3, R: 2, '?': 1 };
            let bestLabel = '';
            let bestPriority = 0;
            for (const [path, s] of gitStatusMap) {
              if (path.startsWith(prefix)) {
                const p = PRIORITY[s.statusLabel] ?? 0;
                if (p > bestPriority) {
                  bestPriority = p;
                  bestLabel = s.statusLabel;
                }
              }
            }
            if (bestLabel) {
              return (
                <span className={`ml-1.5 text-xs font-bold flex-shrink-0 opacity-70 ${GIT_COLORS[bestLabel] ?? 'text-[var(--text-muted)]'}`}>
                  {bestLabel}
                </span>
              );
            }
          }
          return null;
        })()}
      </div>

      {expanded &&
        children.map((child) => (
          <TreeNode
            key={child.path}
            entry={child}
            projectRoot={projectRoot}
            depth={depth + 1}
            gitStatusMap={gitStatusMap}
            onViewDiff={onViewDiff}
            onViewFile={onViewFile}
          />
        ))}
    </div>
  );
}

export function FileTree() {
  const activeProjectId = useAppStore((s) => s.activeProjectId);
  const config = useAppStore((s) => s.config);
  const project = config.projects.find((p) => p.id === activeProjectId);

  const handleOpenInVscode = useCallback(async () => {
    if (!project) return;
    // 前端做一次快速预检查(避免无意义的 invoke),后端会权威地再读一次 config
    if (!(config.vscodePath ?? '').trim()) {
      await message(
        '请先在『设置 → 系统设置 → 外部编辑器』中配置 VS Code 可执行文件路径。',
        { title: '未配置 VS Code 路径', kind: 'warning' },
      );
      return;
    }
    try {
      await invoke('open_in_vscode', { path: project.path });
    } catch (err) {
      const detail = typeof err === 'string' ? err : String(err);
      console.error('打开 VS Code 失败:', err);
      await message(detail, {
        title: '打开 VS Code 失败',
        kind: 'error',
      });
    }
  }, [project, config.vscodePath]);

  const [rootEntries, setRootEntries] = useState<FileEntry[]>([]);
  const [gitStatusMap, setGitStatusMap] = useState<Map<string, GitFileStatus>>(new Map());
  const [diffTarget, setDiffTarget] = useState<GitFileStatus | null>(null);
  const refreshTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const loadGitStatus = useCallback(() => {
    if (!project) return;
    invoke<GitFileStatus[]>('get_git_status', { projectPath: project.path })
      .then((statuses) => {
        const map = new Map<string, GitFileStatus>();
        for (const s of statuses) map.set(s.path, s);
        setGitStatusMap(map);
      })
      .catch(() => setGitStatusMap(new Map()));
  }, [project?.path]);

  useEffect(() => {
    loadGitStatus();
  }, [loadGitStatus]);

  const debouncedRefresh = useCallback(() => {
    if (refreshTimerRef.current) clearTimeout(refreshTimerRef.current);
    refreshTimerRef.current = setTimeout(loadGitStatus, 500);
  }, [loadGitStatus]);

  const loadRootEntries = useCallback(() => {
    if (!project) return;
    invoke<FileEntry[]>('list_directory', {
      projectRoot: project.path,
      path: project.path,
    }).then(setRootEntries);
  }, [project?.path]);

  useEffect(() => {
    if (!project) {
      setRootEntries([]);
      return;
    }
    loadRootEntries();
    invoke('watch_directory', { path: project.path, projectPath: project.path });
    return () => { invoke('unwatch_directory', { path: project.path }); };
  }, [project?.path, loadRootEntries]);

  useTauriEvent<FsChangePayload>('fs-change', useCallback((payload: FsChangePayload) => {
    if (project && payload.path === project.path) {
      loadRootEntries();
    }
  }, [project?.path, loadRootEntries]));

  useTauriEvent<FsChangePayload>('fs-change', useCallback((payload: FsChangePayload) => {
    if (project && payload.projectPath === project.path) {
      debouncedRefresh();
    }
  }, [project?.path, debouncedRefresh]));

  const GIT_PATTERNS = [/create mode/, /Switched to/, /Already up to date/, /insertions?\(\+\)/, /deletions?\(-\)/];
  useTauriEvent<PtyOutputPayload>('pty-output', useCallback((payload: PtyOutputPayload) => {
    if (GIT_PATTERNS.some((p) => p.test(payload.data))) {
      debouncedRefresh();
    }
  }, [debouncedRefresh]));

  const handleViewDiff = useCallback((status: GitFileStatus) => {
    setDiffTarget(status);
  }, []);

  const [viewFilePath, setViewFilePath] = useState<string | null>(null);
  const handleViewFile = useCallback((path: string) => {
    setViewFilePath(path);
  }, []);

  const handleRootContextMenu = useCallback((e: React.MouseEvent) => {
    if (!project) return;
    e.preventDefault();
    const sep = project.path.includes('/') ? '/' : '\\';
    showContextMenu(e.clientX, e.clientY, [
      {
        label: '新建文件',
        onClick: async () => {
          const name = await showPrompt('新建文件', '请输入文件名');
          if (!name?.trim()) return;
          await invoke('create_file', { path: `${project.path}${sep}${name.trim()}` });
          loadRootEntries();
        },
      },
      {
        label: '新建文件夹',
        onClick: async () => {
          const name = await showPrompt('新建文件夹', '请输入文件夹名');
          if (!name?.trim()) return;
          await invoke('create_directory', { path: `${project.path}${sep}${name.trim()}` });
          loadRootEntries();
        },
      },
    ]);
  }, [project, loadRootEntries]);

  if (!project) {
    return (
      <div className="h-full bg-[var(--bg-surface)] flex items-center justify-center text-[var(--text-muted)] text-base">
        选择一个项目
      </div>
    );
  }

  return (
    <div className="h-full bg-[var(--bg-surface)] flex flex-col overflow-y-auto border-l border-[var(--border-subtle)]">
      <div className="px-3 pt-3 pb-1.5 flex items-center justify-between gap-2">
        <span className="text-sm text-[var(--text-muted)] uppercase tracking-[0.12em] font-medium truncate">
          Files — {project.name}
        </span>
        <button
          type="button"
          onClick={handleOpenInVscode}
          title="使用 VS Code 打开该文件夹"
          className="text-[var(--text-muted)] hover:text-[var(--text-primary)] transition-colors flex-shrink-0 leading-none"
        >
          <svg
            width="14"
            height="14"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden="true"
          >
            <path d="M10 2h4v4" />
            <path d="M14 2L7 9" />
            <path d="M12 9v4a1 1 0 01-1 1H3a1 1 0 01-1-1V5a1 1 0 011-1h4" />
          </svg>
        </button>
      </div>
      <div className="flex-1 px-1" onContextMenu={handleRootContextMenu}>
        {rootEntries.map((entry) => (
          <TreeNode
            key={entry.path}
            entry={entry}
            projectRoot={project.path}
            depth={0}
            gitStatusMap={gitStatusMap}
            onViewDiff={handleViewDiff}
            onViewFile={handleViewFile}
          />
        ))}
      </div>
      {viewFilePath && (
        <FileViewerModal
          open={!!viewFilePath}
          onClose={() => setViewFilePath(null)}
          filePath={viewFilePath}
        />
      )}
      {diffTarget && (
        <DiffModal
          open={!!diffTarget}
          onClose={() => setDiffTarget(null)}
          projectPath={project.path}
          status={diffTarget}
        />
      )}
    </div>
  );
}
