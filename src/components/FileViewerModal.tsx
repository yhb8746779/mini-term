import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { FileContentResult } from '../types';

interface FileViewerModalProps {
  open: boolean;
  onClose: () => void;
  filePath: string;
}

export function FileViewerModal({ open, onClose, filePath }: FileViewerModalProps) {
  const [result, setResult] = useState<FileContentResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    if (!open) return;
    setLoading(true);
    setError('');
    setResult(null);

    invoke<FileContentResult>('read_file_content', { path: filePath })
      .then(setResult)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [open, filePath]);

  useEffect(() => {
    if (!open) return;
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [open, onClose]);

  if (!open) return null;

  const fileName = filePath.replace(/\\/g, '/').split('/').pop() ?? filePath;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center" onClick={onClose}>
      <div className="absolute inset-0 bg-black/50 backdrop-blur-sm" />
      <div
        className="relative flex flex-col overflow-hidden bg-[var(--bg-surface)] border border-[var(--border-strong)] rounded-[var(--radius-md)] shadow-2xl animate-slide-in"
        style={{ width: '90vw', height: '80vh' }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* 工具栏 */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--border-subtle)] flex-shrink-0">
          <div className="flex items-center gap-2">
            <span className="text-base font-medium text-[var(--accent)]">{fileName}</span>
            <span className="text-sm text-[var(--text-muted)] truncate max-w-[400px]">
              {filePath}
            </span>
          </div>
          <button
            className="text-[var(--text-muted)] hover:text-[var(--text-primary)] transition-colors text-lg leading-none"
            onClick={onClose}
          >
            ✕
          </button>
        </div>

        {/* 内容区 */}
        <div className="flex-1 overflow-auto bg-[var(--bg-base)]">
          {loading && (
            <div className="flex items-center justify-center h-full text-[var(--text-muted)]">
              加载中...
            </div>
          )}
          {error && (
            <div className="flex items-center justify-center h-full text-[var(--color-error)]">
              {error}
            </div>
          )}
          {result && result.isBinary && (
            <div className="flex items-center justify-center h-full text-[var(--text-muted)]">
              二进制文件，不支持预览
            </div>
          )}
          {result && result.tooLarge && (
            <div className="flex items-center justify-center h-full text-[var(--text-muted)]">
              文件过大（&gt;1MB），不支持预览
            </div>
          )}
          {result && !result.isBinary && !result.tooLarge && (
            <div className="font-mono text-sm leading-6">
              {result.content.split('\n').map((line, i) => (
                <div key={i} className="flex hover:bg-[var(--border-subtle)]">
                  <span className="w-12 text-right pr-3 text-[var(--text-muted)] select-none flex-shrink-0 opacity-40">
                    {i + 1}
                  </span>
                  <span className="flex-1 whitespace-pre px-2 text-[var(--text-primary)]">
                    {line}
                  </span>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
