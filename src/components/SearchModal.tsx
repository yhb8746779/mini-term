import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useAppStore } from '../store';
import { useTauriEvent } from '../hooks/useTauriEvent';
import { FileViewerModal } from './FileViewerModal';
import type { SearchResultItem, SearchResultsPayload, SearchCompletePayload } from '../types';

// ── Keyword highlight helper ──

function HighlightText({ text, ranges }: { text: string; ranges: [number, number][] }) {
  if (ranges.length === 0) return <>{text}</>;
  const parts: React.ReactNode[] = [];
  let lastEnd = 0;
  for (const [start, end] of ranges) {
    if (start > lastEnd) {
      parts.push(text.slice(lastEnd, start));
    }
    parts.push(
      <span key={start} className="bg-[var(--color-warning)]/30 text-[var(--color-warning)] rounded-sm px-[1px]">
        {text.slice(start, end)}
      </span>,
    );
    lastEnd = end;
  }
  if (lastEnd < text.length) {
    parts.push(text.slice(lastEnd));
  }
  return <>{parts}</>;
}

// ── Content results grouped by file ──

function ContentResults({
  results,
  onResultClick,
  onResultDoubleClick,
}: {
  results: SearchResultItem[];
  onResultClick: (item: SearchResultItem) => void;
  onResultDoubleClick: (item: SearchResultItem) => void;
}) {
  const grouped = new Map<string, SearchResultItem[]>();
  for (const item of results) {
    const group = grouped.get(item.filePath) ?? [];
    group.push(item);
    grouped.set(item.filePath, group);
  }

  return (
    <>
      {Array.from(grouped.entries()).map(([filePath, items]) => (
        <div key={filePath}>
          <div className="px-4 py-1.5 text-xs text-[var(--accent)] bg-[var(--bg-elevated)] font-medium sticky top-0 z-10 flex items-center gap-2">
            <span>{items[0].fileName}</span>
            <span className="text-[var(--text-muted)] truncate">{filePath}</span>
            <span className="text-[var(--text-muted)]">({items.length})</span>
          </div>
          {items.map((item, i) => (
            <div
              key={i}
              className="flex items-center gap-2 px-4 py-1 cursor-pointer hover:bg-[var(--border-subtle)] transition-colors font-mono text-xs"
              onClick={() => onResultClick(item)}
              onDoubleClick={() => onResultDoubleClick(item)}
            >
              <span className="w-10 text-right text-[var(--text-muted)] flex-shrink-0 select-none">
                {item.lineNumber}
              </span>
              <span className="text-[var(--text-primary)] truncate">
                <HighlightText text={item.lineContent ?? ''} ranges={item.matchRanges} />
              </span>
            </div>
          ))}
        </div>
      ))}
    </>
  );
}

// ── SearchModal ──

interface SearchModalProps {
  open: boolean;
  onClose: () => void;
}

export function SearchModal({ open, onClose }: SearchModalProps) {
  const [query, setQuery] = useState('');
  const [mode, setMode] = useState<'filename' | 'content'>('filename');
  const [useRegex, setUseRegex] = useState(false);
  const [searchId, setSearchId] = useState<string | null>(null);
  const [results, setResults] = useState<SearchResultItem[]>([]);
  const [status, setStatus] = useState<'idle' | 'searching' | 'done'>('idle');
  const [totalCount, setTotalCount] = useState(0);
  const [viewFilePath, setViewFilePath] = useState<string | null>(null);
  const [viewHighlightLine, setViewHighlightLine] = useState<number | undefined>();
  const inputRef = useRef<HTMLInputElement>(null);
  const searchIdRef = useRef<string | null>(null);

  const activeProjectId = useAppStore((s) => s.activeProjectId);
  const config = useAppStore((s) => s.config);
  const project = config.projects.find((p) => p.id === activeProjectId);

  // Keep ref in sync so event listeners always see the latest value
  searchIdRef.current = searchId;

  // Focus input when modal opens; reset state on close
  useEffect(() => {
    if (open) {
      setTimeout(() => inputRef.current?.focus(), 50);
    } else {
      if (searchIdRef.current) {
        invoke('cancel_search', { searchId: searchIdRef.current }).catch(() => {});
      }
      setQuery('');
      setResults([]);
      setStatus('idle');
      setTotalCount(0);
      setSearchId(null);
    }
  }, [open]);

  // Clear results when mode changes
  useEffect(() => {
    if (searchIdRef.current) {
      invoke('cancel_search', { searchId: searchIdRef.current }).catch(() => {});
    }
    setResults([]);
    setStatus('idle');
    setTotalCount(0);
    setSearchId(null);
  }, [mode]);

  // Escape to close (only when FileViewerModal is not open)
  useEffect(() => {
    if (!open) return;
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !viewFilePath) {
        onClose();
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [open, onClose, viewFilePath]);

  // Listen for search results
  useTauriEvent<SearchResultsPayload>(
    'search-results',
    useCallback((payload) => {
      if (payload.searchId !== searchIdRef.current) return;
      setResults((prev) => {
        if (prev.length >= 1000) return prev;
        const remaining = 1000 - prev.length;
        return [...prev, ...payload.items.slice(0, remaining)];
      });
    }, []),
  );

  // Listen for search complete
  useTauriEvent<SearchCompletePayload>(
    'search-complete',
    useCallback((payload) => {
      if (payload.searchId !== searchIdRef.current) return;
      setStatus('done');
      setTotalCount(payload.totalCount);
    }, []),
  );

  const handleSearch = useCallback(() => {
    if (!query.trim() || !project) return;
    if (searchIdRef.current) {
      invoke('cancel_search', { searchId: searchIdRef.current }).catch(() => {});
    }
    const newId = crypto.randomUUID();
    searchIdRef.current = newId;
    setSearchId(newId);
    setResults([]);
    setStatus('searching');
    setTotalCount(0);
    invoke('start_search', {
      projectRoot: project.path,
      query: query.trim(),
      mode,
      useRegex,
      searchId: newId,
    }).catch(() => setStatus('done'));
  }, [query, project, mode, useRegex]);

  const handleResultClick = useCallback(
    (item: SearchResultItem) => {
      if (!project) return;
      const sep = project.path.includes('\\') ? '\\' : '/';
      setViewFilePath(project.path + sep + item.filePath);
      setViewHighlightLine(item.lineNumber ?? undefined);
    },
    [project],
  );

  const handleResultDoubleClick = useCallback(
    (item: SearchResultItem) => {
      if (!project) return;
      const sep = project.path.includes('\\') ? '\\' : '/';
      invoke('open_in_vscode', {
        path: project.path + sep + item.filePath,
      }).catch(() => {});
    },
    [project],
  );

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center select-text">
      <div className="absolute inset-0 bg-black/50 backdrop-blur-sm" />
      <div
        className="relative flex flex-col overflow-hidden bg-[var(--bg-surface)] border border-[var(--border-strong)] rounded-[var(--radius-md)] shadow-[var(--shadow-overlay)] animate-slide-in"
        style={{ width: '80vw', height: '70vh', maxWidth: '900px' }}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--border-subtle)] flex-shrink-0">
          <div className="flex items-center gap-3">
            <span className="text-base font-medium text-[var(--accent)]">搜索</span>
            <div className="flex rounded-[var(--radius-sm)] border border-[var(--border-default)] overflow-hidden text-xs">
              <button
                className={`px-2.5 py-1 transition-colors ${mode === 'filename' ? 'bg-[var(--accent)] text-[var(--bg-base)]' : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'}`}
                onClick={() => setMode('filename')}
              >
                文件名
              </button>
              <button
                className={`px-2.5 py-1 transition-colors ${mode === 'content' ? 'bg-[var(--accent)] text-[var(--bg-base)]' : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'}`}
                onClick={() => setMode('content')}
              >
                内容
              </button>
            </div>
          </div>
          <button
            className="text-[var(--text-muted)] hover:text-[var(--text-primary)] transition-colors text-lg leading-none"
            onClick={onClose}
          >
            ✕
          </button>
        </div>

        {/* Search input */}
        <div className="flex items-center gap-2 px-4 py-2 border-b border-[var(--border-subtle)]">
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') handleSearch();
            }}
            placeholder={mode === 'filename' ? '搜索文件名...' : '搜索文件内容...'}
            className="flex-1 bg-[var(--bg-base)] border border-[var(--border-default)] rounded-[var(--radius-sm)] px-3 py-1.5 text-sm text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
          />
          <button
            className={`px-2 py-1.5 text-xs rounded-[var(--radius-sm)] border transition-colors font-mono ${
              useRegex
                ? 'bg-[var(--accent)] text-[var(--bg-base)] border-[var(--accent)]'
                : 'text-[var(--text-muted)] border-[var(--border-default)] hover:text-[var(--text-primary)]'
            }`}
            onClick={() => setUseRegex(!useRegex)}
            title="正则表达式"
          >
            .*
          </button>
          <button
            onClick={handleSearch}
            disabled={!query.trim() || status === 'searching'}
            className="px-3 py-1.5 text-xs rounded-[var(--radius-sm)] bg-[var(--accent)] text-[var(--bg-base)] hover:opacity-90 disabled:opacity-50 transition-opacity"
          >
            搜索
          </button>
        </div>

        {/* Results */}
        <div className="flex-1 overflow-auto bg-[var(--bg-base)]">
          {status === 'searching' && results.length === 0 && (
            <div className="flex items-center justify-center h-full text-[var(--text-muted)]">搜索中...</div>
          )}
          {status === 'idle' && (
            <div className="flex items-center justify-center h-full text-[var(--text-muted)] text-sm">
              输入关键词后按 Enter 开始搜索
            </div>
          )}
          {results.length > 0 && (
            <div className="divide-y divide-[var(--border-subtle)]">
              {mode === 'filename'
                ? results.map((item, i) => (
                    <div
                      key={i}
                      className="flex items-center gap-2 px-4 py-1.5 cursor-pointer hover:bg-[var(--border-subtle)] transition-colors"
                      onClick={() => handleResultClick(item)}
                      onDoubleClick={() => handleResultDoubleClick(item)}
                    >
                      <span className="text-sm text-[var(--text-primary)]">
                        <HighlightText text={item.fileName} ranges={item.matchRanges} />
                      </span>
                      <span className="text-xs text-[var(--text-muted)] truncate">{item.filePath}</span>
                    </div>
                  ))
                : (
                    <ContentResults results={results} onResultClick={handleResultClick} onResultDoubleClick={handleResultDoubleClick} />
                  )}
            </div>
          )}
          {results.length >= 1000 && (
            <div className="px-4 py-2 text-xs text-[var(--color-warning)] bg-[var(--bg-elevated)]">
              已显示前 1000 条结果，更多结果未显示
            </div>
          )}
        </div>

        {/* Status bar */}
        <div className="flex items-center px-4 py-1.5 border-t border-[var(--border-subtle)] text-xs text-[var(--text-muted)] flex-shrink-0">
          {status === 'searching' && <span>搜索中... 已找到 {results.length} 条</span>}
          {status === 'done' && mode === 'filename' && <span>找到 {totalCount} 个文件</span>}
          {status === 'done' && mode === 'content' && <span>找到 {totalCount} 处匹配</span>}
          {status === 'idle' && <span>Ctrl+Shift+F 打开搜索</span>}
        </div>
      </div>

      {viewFilePath && project && (
        <div onClick={(e) => e.stopPropagation()}>
          <FileViewerModal
            open={!!viewFilePath}
            onClose={() => setViewFilePath(null)}
            filePath={viewFilePath}
            projectRoot={project.path}
            highlightLine={viewHighlightLine}
          />
        </div>
      )}
    </div>
  );
}
