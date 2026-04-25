import { useEffect, useState, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useAppStore } from '../store';
import { showContextMenu } from '../utils/contextMenu';
import type { AiSession } from '../types';

interface SessionsResult {
  sessions: AiSession[];
  /** true = 后台扫描进行中，稍后 sessions-updated 到达才是最终数据 */
  scanning: boolean;
}

/** 将 ISO 时间戳转换为简短的相对/绝对时间 */
function formatTime(iso: string): string {
  if (!iso) return '';
  const date = new Date(iso);
  if (isNaN(date.getTime())) return '';

  const now = Date.now();
  const diff = now - date.getTime();
  const minutes = Math.floor(diff / 60000);

  if (minutes < 1) return '刚刚';
  if (minutes < 60) return `${minutes}分钟前`;

  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}小时前`;

  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}天前`;

  // 超过一周显示日期
  const m = date.getMonth() + 1;
  const d = date.getDate();
  const y = date.getFullYear();
  const currentYear = new Date().getFullYear();
  return y === currentYear ? `${m}月${d}日` : `${y}/${m}/${d}`;
}

// 与 StatusDot 保持一致：claude=橙、codex=蓝、gemini=绿、grok=紫。
// Provider 身份色现在是 status-无关的单值变量（--color-{provider}），与 StatusDot 共用同一组源。
const TYPE_BADGE: Record<string, { label: string; color: string }> = {
  claude: { label: 'C', color: 'var(--color-claude)' },
  codex:  { label: 'X', color: 'var(--color-codex)' },
  gemini: { label: 'G', color: 'var(--color-gemini)' },
  grok:   { label: 'K', color: 'var(--color-grok)' },
};

export function SessionList() {
  const config = useAppStore((s) => s.config);
  const activeProjectId = useAppStore((s) => s.activeProjectId);

  const [sessions, setSessions] = useState<AiSession[]>([]);
  const [loading, setLoading] = useState(false);

  const activeProject = config.projects.find((p) => p.id === activeProjectId);

  const fetchSessions = useCallback(async (projectPath: string, force = false) => {
    setLoading(true);
    try {
      const { sessions: result, scanning } = await invoke<SessionsResult>('get_ai_sessions', { projectPath, force });
      setSessions(result);
      if (scanning) {
        // 后台扫描中：保持 loading=true，等待 sessions-updated 事件到达后再清除
        return;
      }
    } catch {
      setSessions([]);
    }
    setLoading(false);
  }, []);

  // 切换项目时拉取
  useEffect(() => {
    if (activeProject?.path) {
      fetchSessions(activeProject.path);
    } else {
      setSessions([]);
      setLoading(false);
    }
  }, [activeProject?.path, fetchSessions]);

  // 后台扫描完成后更新当前项目的 session 列表
  useEffect(() => {
    if (!activeProject?.path) return;
    let unlisten: (() => void) | undefined;
    listen<string>('sessions-updated', (event) => {
      if (event.payload === activeProject?.path) {
        fetchSessions(activeProject.path); // 命中新鲜缓存，scanning=false，立即结束 loading
      }
    }).then((fn) => { unlisten = fn; });
    return () => { unlisten?.(); };
  }, [activeProject?.path, fetchSessions]);

  return (
    <div className="h-full flex flex-col overflow-hidden bg-[var(--bg-surface)]">
      <div className="px-3 pt-2.5 pb-1.5 text-sm text-[var(--text-muted)] uppercase tracking-[0.12em] font-medium flex items-center justify-between">
        <span>Sessions</span>
        {activeProject && (
          <span
            className="text-xs normal-case tracking-normal cursor-pointer hover:text-[var(--text-primary)] transition-colors"
            onClick={() => fetchSessions(activeProject.path, true)}
            title="刷新会话列表"
          >
            ↻
          </span>
        )}
      </div>

      <div className="flex-1 overflow-y-auto px-1.5">
        {loading && sessions.length === 0 && (
          <div className="px-2.5 py-3 text-xs text-[var(--text-muted)] text-center">加载中…</div>
        )}

        {!loading && sessions.length === 0 && (
          <div className="px-2.5 py-3 text-xs text-[var(--text-muted)] text-center">
            {activeProject ? '暂无会话记录' : '请先选择项目'}
          </div>
        )}

        {sessions.map((session) => {
          const badge = TYPE_BADGE[session.sessionType] ?? TYPE_BADGE.claude;

          return (
            <div
              key={`${session.sessionType}-${session.id}`}
              className="flex items-start gap-2 px-2.5 py-1.5 rounded-[var(--radius-sm)] text-xs group hover:bg-[var(--border-subtle)] transition-colors cursor-default"
              title={session.title}
              onContextMenu={(e) => {
                e.preventDefault();
                e.stopPropagation();
                const cmd = session.sessionType === 'claude'
                  ? `claude --resume ${session.id}`
                  : `codex resume ${session.id}`;
                showContextMenu(e.clientX, e.clientY, [
                  {
                    label: '复制恢复命令',
                    onClick: () => navigator.clipboard.writeText(cmd),
                  },
                ]);
              }}
            >
              {/* 类型徽标 */}
              <span
                className="flex-shrink-0 w-4 h-4 rounded flex items-center justify-center text-[10px] font-bold mt-0.5"
                style={{ backgroundColor: badge.color + '22', color: badge.color }}
              >
                {badge.label}
              </span>

              {/* 标题 + 时间 */}
              <div className="flex-1 min-w-0">
                <div className="truncate text-[var(--text-secondary)] group-hover:text-[var(--text-primary)] transition-colors leading-snug">
                  {session.title}
                </div>
                <div className="text-[var(--text-muted)] text-[10px] mt-0.5 leading-none">
                  {formatTime(session.timestamp)}
                </div>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
