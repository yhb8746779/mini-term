import { useState, useEffect } from 'react';
import type { PaneStatus, AiProvider } from '../types';

/**
 * 状态点视觉规范（Provider 身份与 Status 活动彻底解耦）：
 *
 * 维度 A — Provider 身份（一种 provider = 一个 base 色）：
 *   PROVIDER_COLORS 在 :root 里定义为 CSS 变量，单值，无 complete/generating 之分。
 *
 * 维度 B — Status 活动，三种独立机制叠加在 base 色上：
 *   1) STATUS_OVERRIDES：idle/error 直接覆盖 base（脱离 provider 语义）
 *   2) GENERATING_BRIGHTEN：ai-generating 时把 base 色加白 25%（亮度替代动效）
 *   3) STATUS_ANIMATIONS：
 *      - ai-generating 用 pulse-fast 0.8s 快闪（节奏明显，表达正在输出 token）
 *      - ai-thinking 用 pulse-slow 1.8s 慢呼吸（温和，表达稳态思考）
 *      - ai-awaiting-input 走 React 定时器在 base 色 ↔ WARN 黄之间切换
 *
 * 任何 provider 改色只动 PROVIDER_COLORS（实际是动 styles.css 的 4 个变量）；
 * 任何状态改动效只动 STATUS_OVERRIDES / STATUS_ANIMATIONS / GENERATING_BRIGHTEN。
 * 两个维度互不影响。
 */

const PROVIDER_COLORS: Record<AiProvider, string> = {
  claude: 'var(--color-claude)',
  codex:  'var(--color-codex)',
  gemini: 'var(--color-gemini)',
  grok:   'var(--color-grok)',
};

/** idle / error 这类与 provider 无关的状态，直接覆盖颜色 */
const STATUS_OVERRIDES: Partial<Record<PaneStatus, string>> = {
  idle:  'var(--text-muted)',
  error: 'var(--color-error)',
};

/** 三档动效区分活动状态：generating=快闪 / thinking=慢呼吸 / awaiting=黄色切换（在组件内 React 定时器） */
const STATUS_ANIMATIONS: Partial<Record<PaneStatus, string>> = {
  'ai-generating': 'animate-pulse-fast',
  'ai-thinking':   'animate-pulse-slow',
};

/** ai-generating 时把 provider 色加白 25%，亮度替代动效，凸显"正在输出" */
const GENERATING_BRIGHTEN_PCT = 25;

const WARN_COLOR = '#f5c518';

function resolveBaseColor(status: PaneStatus, provider?: AiProvider): string {
  // idle / error 覆盖优先（不看 provider）
  const override = STATUS_OVERRIDES[status];
  if (override) return override;

  // 进入 AI 状态但 provider 还没识别出来，用静态灰兜底
  const providerColor = provider ? PROVIDER_COLORS[provider] : 'var(--text-muted)';

  // ai-generating 加白增亮（CSS color-mix 现代浏览器/WebView 均支持）
  if (status === 'ai-generating' && provider) {
    return `color-mix(in srgb, ${providerColor}, white ${GENERATING_BRIGHTEN_PCT}%)`;
  }

  return providerColor;
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
  // ai-awaiting-input 时每 250ms 在 base 色 ↔ WARN 黄之间切换
  const isAwaiting = status === 'ai-awaiting-input';
  const [altPhase, setAltPhase] = useState(false);
  useEffect(() => {
    if (!isAwaiting) { setAltPhase(false); return; }
    const id = setInterval(() => setAltPhase((v) => !v), 250);
    return () => clearInterval(id);
  }, [isAwaiting]);

  const baseColor = resolveBaseColor(status, provider);
  const bgColor = isAwaiting && altPhase ? WARN_COLOR : baseColor;

  const animClass = STATUS_ANIMATIONS[status] ?? '';
  const dim = size === 'sm' ? 'w-1.5 h-1.5' : 'w-2 h-2';

  return (
    <span
      className={`inline-block rounded-full flex-shrink-0 ${dim} ${animClass}`}
      style={{ backgroundColor: bgColor }}
      title={getTooltip(status, provider)}
    />
  );
}
