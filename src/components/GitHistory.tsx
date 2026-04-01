import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import { useAppStore } from '../store';
import { useTauriEvent } from '../hooks/useTauriEvent';
import { showContextMenu } from '../utils/contextMenu';
import { formatRelativeTime } from '../utils/timeFormat';
import { CommitDiffModal } from './CommitDiffModal';
import type { GitRepoInfo, GitCommitInfo, CommitFileInfo, PtyOutputPayload } from '../types';

interface RepoState {
  commits: GitCommitInfo[];
  loading: boolean;
  hasMore: boolean;
}

const GIT_REFRESH_PATTERNS = [
  /create mode/,
  /Switched to/,
  /Already up to date/,
  /insertions?\(\+\)/,
  /deletions?\(-\)/,
];

export function GitHistory() {
  const activeProjectId = useAppStore((s) => s.activeProjectId);
  const config = useAppStore((s) => s.config);
  const project = config.projects.find((p) => p.id === activeProjectId);

  const [repos, setRepos] = useState<GitRepoInfo[]>([]);
  const [expandedRepos, setExpandedRepos] = useState<Set<string>>(new Set());
  const [repoStates, setRepoStates] = useState<Map<string, RepoState>>(new Map());
  const [diffModal, setDiffModal] = useState<{
    open: boolean;
    repoPath: string;
    commitHash: string;
    commitMessage: string;
    files: CommitFileInfo[];
  } | null>(null);

  const scrollRef = useRef<HTMLDivElement>(null);
  const repoStatesRef = useRef(repoStates);
  repoStatesRef.current = repoStates;

  const loadRepos = useCallback(() => {
    if (!project) return;
    invoke<GitRepoInfo[]>('discover_git_repos', { projectPath: project.path })
      .then(setRepos)
      .catch(() => setRepos([]));
  }, [project?.path]);

  useEffect(() => {
    loadRepos();
  }, [loadRepos]);

  const loadCommits = useCallback(
    async (repoPath: string, beforeCommit?: string) => {
      const existing = repoStatesRef.current.get(repoPath);
      if (existing?.loading) return;

      setRepoStates((prev) => {
        const next = new Map(prev);
        const cur = next.get(repoPath) ?? { commits: [], loading: false, hasMore: true };
        next.set(repoPath, { ...cur, loading: true });
        return next;
      });

      try {
        const commits = await invoke<GitCommitInfo[]>('get_git_log', {
          repoPath,
          beforeCommit: beforeCommit ?? null,
          limit: 30,
        });
        setRepoStates((prev) => {
          const next = new Map(prev);
          const cur = next.get(repoPath) ?? { commits: [], loading: false, hasMore: true };
          next.set(repoPath, {
            commits: beforeCommit ? [...cur.commits, ...commits] : commits,
            loading: false,
            hasMore: commits.length >= 30,
          });
          return next;
        });
      } catch {
        setRepoStates((prev) => {
          const next = new Map(prev);
          const cur = next.get(repoPath) ?? { commits: [], loading: false, hasMore: true };
          next.set(repoPath, { ...cur, loading: false });
          return next;
        });
      }
    },
    [],
  );

  const toggleRepo = useCallback(
    (repoPath: string) => {
      setExpandedRepos((prev) => {
        const next = new Set(prev);
        if (next.has(repoPath)) {
          next.delete(repoPath);
        } else {
          next.add(repoPath);
          if (!repoStatesRef.current.has(repoPath)) {
            loadCommits(repoPath);
          }
        }
        return next;
      });
    },
    [loadCommits],
  );

  const expandedReposRef = useRef(expandedRepos);
  expandedReposRef.current = expandedRepos;

  const handleScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    if (el.scrollTop + el.clientHeight < el.scrollHeight - 50) return;

    for (const repoPath of expandedReposRef.current) {
      const state = repoStatesRef.current.get(repoPath);
      if (state && state.hasMore && !state.loading && state.commits.length > 0) {
        const lastHash = state.commits[state.commits.length - 1].hash;
        loadCommits(repoPath, lastHash);
        break;
      }
    }
  }, [loadCommits]);

  const handleViewDiff = useCallback(async (repoPath: string, commit: GitCommitInfo) => {
    try {
      const files = await invoke<CommitFileInfo[]>('get_commit_files', {
        repoPath,
        commitHash: commit.hash,
      });
      setDiffModal({
        open: true,
        repoPath,
        commitHash: commit.hash,
        commitMessage: commit.message,
        files,
      });
    } catch (e) {
      console.error('get_commit_files failed:', e);
    }
  }, []);

  const handleCommitContextMenu = useCallback(
    (e: React.MouseEvent, repoPath: string, commit: GitCommitInfo) => {
      e.preventDefault();
      showContextMenu(e.clientX, e.clientY, [
        {
          label: '复制 Commit Hash',
          onClick: () => writeText(commit.hash),
        },
        { separator: true },
        {
          label: '查看变更',
          onClick: () => handleViewDiff(repoPath, commit),
        },
      ]);
    },
    [handleViewDiff],
  );

  const refreshTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const debouncedRefresh = useCallback(() => {
    if (refreshTimerRef.current) clearTimeout(refreshTimerRef.current);
    refreshTimerRef.current = setTimeout(() => {
      loadRepos();
      for (const repoPath of expandedReposRef.current) {
        loadCommits(repoPath);
      }
    }, 500);
  }, [loadRepos, loadCommits]);

  useTauriEvent<PtyOutputPayload>(
    'pty-output',
    useCallback(
      (payload: PtyOutputPayload) => {
        if (GIT_REFRESH_PATTERNS.some((p) => p.test(payload.data))) {
          debouncedRefresh();
        }
      },
      [debouncedRefresh],
    ),
  );

  if (!project) {
    return (
      <div className="h-full bg-[var(--bg-surface)] flex items-center justify-center text-[var(--text-muted)] text-base">
        选择一个项目
      </div>
    );
  }

  return (
    <div className="h-full bg-[var(--bg-surface)] flex flex-col border-t border-[var(--border-subtle)]">
      <div className="flex items-center justify-between px-3 pt-3 pb-1.5">
        <span className="text-sm text-[var(--text-muted)] uppercase tracking-[0.12em] font-medium">
          Git History
        </span>
        <button
          className="text-[var(--text-muted)] hover:text-[var(--text-primary)] transition-colors text-sm"
          onClick={() => {
            loadRepos();
            for (const repoPath of expandedRepos) {
              loadCommits(repoPath);
            }
          }}
          title="刷新"
        >
          ↻
        </button>
      </div>

      <div className="flex-1 overflow-y-auto px-1" ref={scrollRef} onScroll={handleScroll}>
        {repos.length === 0 && (
          <div className="text-center text-[var(--text-muted)] text-sm py-6">
            未发现 Git 仓库
          </div>
        )}

        {repos.map((repo) => {
          const isExpanded = expandedRepos.has(repo.path);
          const state = repoStates.get(repo.path);
          return (
            <div key={repo.path}>
              <div
                className="flex items-center gap-1 py-[5px] px-2 cursor-pointer hover:bg-[var(--border-subtle)] rounded-[var(--radius-sm)] text-base transition-colors duration-100 text-[var(--color-folder)]"
                onClick={() => toggleRepo(repo.path)}
              >
                <span
                  className="text-[13px] w-3 text-center text-[var(--text-muted)] transition-transform duration-150"
                  style={{
                    transform: isExpanded ? 'rotate(0deg)' : 'rotate(-90deg)',
                    display: 'inline-block',
                  }}
                >
                  &#9662;
                </span>
                <span className="truncate font-medium">{repo.name}</span>
              </div>

              {isExpanded && (
                <div className="ml-4">
                  {state?.commits.map((commit) => (
                    <div
                      key={commit.hash}
                      className="py-1.5 px-2 cursor-pointer hover:bg-[var(--border-subtle)] rounded-[var(--radius-sm)] transition-colors duration-100"
                      onContextMenu={(e) => handleCommitContextMenu(e, repo.path, commit)}
                      onDoubleClick={() => handleViewDiff(repo.path, commit)}
                    >
                      <div className="text-sm text-[var(--text-primary)] truncate">
                        {commit.message}
                      </div>
                      <div className="text-xs text-[var(--text-muted)] flex items-center gap-1.5 mt-0.5">
                        <span>{commit.author}</span>
                        <span>&middot;</span>
                        <span>{formatRelativeTime(commit.timestamp)}</span>
                        <span>&middot;</span>
                        <span className="font-mono">{commit.shortHash}</span>
                      </div>
                    </div>
                  ))}

                  {state?.loading && (
                    <div className="text-center text-[var(--text-muted)] text-xs py-2">
                      加载中...
                    </div>
                  )}

                  {state && !state.loading && state.commits.length === 0 && (
                    <div className="text-center text-[var(--text-muted)] text-xs py-2">
                      暂无提交
                    </div>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>

      {diffModal && (
        <CommitDiffModal
          open={diffModal.open}
          onClose={() => setDiffModal(null)}
          repoPath={diffModal.repoPath}
          commitHash={diffModal.commitHash}
          commitMessage={diffModal.commitMessage}
          files={diffModal.files}
        />
      )}
    </div>
  );
}
