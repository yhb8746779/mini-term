/**
 * 轻量级错误 toast：不走 zustand store / React，直接挂 DOM。
 *
 * 与 ToastContainer（AI 完成通知）共存：定位在左下角避开右下角的 AI toast 栈。
 * 主要用于 invoke 失败等同步反馈场景，无需进入应用数据模型。
 */

const STACK_ID = 'error-toast-stack';
const TOAST_LIFETIME_MS = 8000;

function ensureStack(): HTMLDivElement {
  let stack = document.getElementById(STACK_ID) as HTMLDivElement | null;
  if (stack) return stack;
  stack = document.createElement('div');
  stack.id = STACK_ID;
  Object.assign(stack.style, {
    position: 'fixed',
    left: '16px',
    bottom: '16px',
    display: 'flex',
    flexDirection: 'column',
    gap: '8px',
    zIndex: '9999',
    pointerEvents: 'none',
  } satisfies Partial<CSSStyleDeclaration>);
  document.body.appendChild(stack);
  return stack;
}

export function showErrorToast(message: string): void {
  const stack = ensureStack();

  const card = document.createElement('div');
  Object.assign(card.style, {
    minWidth: '280px',
    maxWidth: '420px',
    background: 'var(--bg-elevated, #2a2a2a)',
    color: 'var(--text-primary, #f0f0f0)',
    border: '1px solid var(--border-default, #444)',
    borderLeft: '3px solid #e5484d',
    borderRadius: '6px',
    padding: '10px 12px',
    fontSize: '12px',
    lineHeight: '1.5',
    boxShadow: '0 4px 12px rgba(0, 0, 0, 0.35)',
    display: 'flex',
    alignItems: 'flex-start',
    gap: '8px',
    pointerEvents: 'auto',
    animation: 'toastSlideIn 0.18s ease-out',
  } satisfies Partial<CSSStyleDeclaration>);

  const icon = document.createElement('div');
  icon.textContent = '!';
  Object.assign(icon.style, {
    flex: '0 0 18px',
    width: '18px',
    height: '18px',
    borderRadius: '50%',
    background: '#e5484d',
    color: '#fff',
    fontWeight: '700',
    fontSize: '12px',
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    marginTop: '1px',
  } satisfies Partial<CSSStyleDeclaration>);

  const body = document.createElement('div');
  body.textContent = message;
  Object.assign(body.style, {
    flex: '1 1 auto',
    wordBreak: 'break-word',
  } satisfies Partial<CSSStyleDeclaration>);

  const close = document.createElement('div');
  close.textContent = '×';
  Object.assign(close.style, {
    flex: '0 0 auto',
    cursor: 'pointer',
    opacity: '0.6',
    fontSize: '16px',
    lineHeight: '1',
    padding: '0 4px',
  } satisfies Partial<CSSStyleDeclaration>);

  let removed = false;
  const remove = () => {
    if (removed) return;
    removed = true;
    card.remove();
    if (stack.childElementCount === 0) stack.remove();
  };

  close.addEventListener('click', remove);
  card.appendChild(icon);
  card.appendChild(body);
  card.appendChild(close);
  stack.appendChild(card);
  setTimeout(remove, TOAST_LIFETIME_MS);
}
