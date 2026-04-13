import { useState, useEffect } from 'react';
import type { PaneStatus, AiProvider } from '../types';

const PROVIDER_COLORS: Record<AiProvider, { complete: string; generating: string }> = {
  claude: { complete: '#d98b3a', generating: '#f2a65a' },
  codex:  { complete: '#2f89c5', generating: '#4db6ff' },
  gemini: { complete: '#2ea56b', generating: '#45c486' },
};

const WARN_COLOR = '#f5c518';

function getBaseColor(status: PaneStatus, provider?: AiProvider): string {
  if (status === 'idle') return 'var(--text-muted)';
  if (status === 'error') return 'var(--color-error)';
  if (!provider) return 'var(--text-muted)';
  const colors = PROVIDER_COLORS[provider];
  if (status === 'ai-complete') return colors.complete;
  if (status === 'ai-generating') return colors.generating;
  if (status === 'ai-thinking') return colors.generating;   // same bright color, but steady (no blink)
  if (status === 'ai-awaiting-input') return colors.complete; // will alternate with WARN_COLOR
  return 'var(--text-muted)';
}

function getTooltip(status: PaneStatus, provider?: AiProvider): string {
  if (status === 'idle') return '空闲';
  if (status === 'error') return '错误';
  const name = provider ? provider.charAt(0).toUpperCase() + provider.slice(1) : 'AI';
  switch (status) {
    case 'ai-generating':     return `${name} · 输出中`;
    case 'ai-thinking':       return `${name} · 思考/工具调用中`;
    case 'ai-awaiting-input': return `${name} · 等待你操作`;
    case 'ai-complete':       return `${name} · 已完成，等待下一条指令`;
    default: return status;
  }
}

export function StatusDot({
  status,
  provider,
  size = 'sm',
}: {
  status: PaneStatus;
  provider?: AiProvider;
  size?: 'sm' | 'md';
}) {
  // 交替闪烁：ai-awaiting-input 时每 250ms 在 provider 色 和 警告黄 之间切换
  const isAwaiting = status === 'ai-awaiting-input';
  const [altPhase, setAltPhase] = useState(false);
  useEffect(() => {
    if (!isAwaiting) { setAltPhase(false); return; }
    const id = setInterval(() => setAltPhase((v) => !v), 250);
    return () => clearInterval(id);
  }, [isAwaiting]);

  const baseColor = getBaseColor(status, provider);
  const bgColor = isAwaiting && altPhase ? WARN_COLOR : baseColor;

  const anim = status === 'ai-generating' ? 'animate-blink-slow' : '';
  const dim = size === 'sm' ? 'w-1.5 h-1.5' : 'w-2 h-2';

  return (
    <span
      className={`inline-block rounded-full flex-shrink-0 ${dim} ${anim}`}
      style={{ backgroundColor: bgColor }}
      title={getTooltip(status, provider)}
    />
  );
}
