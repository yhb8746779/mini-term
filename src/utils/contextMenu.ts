interface MenuItem {
  label: string;
  danger?: boolean;
  disabled?: boolean;
  onClick: () => void;
}

interface MenuSeparator {
  separator: true;
}

type MenuEntry = MenuItem | MenuSeparator;

// 模块级变量:追踪当前活跃菜单的 cleanup 函数。
// 通过 `currentCleanup === cleanup` 同时承担"是否仍是当前菜单"与"是否已被清理"两个判断,
// 避免额外的 cleanedUp 布尔标志。
let currentCleanup: (() => void) | null = null;

export function showContextMenu(x: number, y: number, items: MenuEntry[]) {
  // 先关闭上一个菜单(DOM + document listener 一并清理)
  if (currentCleanup) {
    currentCleanup();
  }

  const menu = document.createElement('div');
  menu.className = 'fixed ctx-menu text-xs';
  menu.style.left = `${x}px`;
  menu.style.top = `${y}px`;

  // 先声明 cleanup/onKey,再构建菜单项,避免 item.onclick 里前向引用
  const cleanup = () => {
    // 已被替换或清理 → 幂等返回
    if (currentCleanup !== cleanup) return;
    currentCleanup = null;
    menu.remove();
    document.removeEventListener('click', cleanup);
    document.removeEventListener('keydown', onKey);
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Escape') cleanup();
  };

  items.forEach((entry) => {
    if ('separator' in entry) {
      const sep = document.createElement('div');
      sep.className = 'ctx-menu-sep';
      menu.appendChild(sep);
      return;
    }
    const item = document.createElement('div');
    const classes = ['ctx-menu-item'];
    if (entry.danger) classes.push('danger');
    if (entry.disabled) classes.push('disabled');
    item.className = classes.join(' ');
    item.textContent = entry.label;
    item.onclick = () => {
      if (entry.disabled) return;
      entry.onClick();
      cleanup();
    };
    menu.appendChild(item);
  });

  document.body.appendChild(menu);

  currentCleanup = cleanup;

  // 延迟一帧注册,避免当前鼠标事件冒泡到 document 立刻触发 cleanup
  setTimeout(() => {
    // 如果在排队期间已被替换或清理,不再注册
    if (currentCleanup !== cleanup) return;
    document.addEventListener('click', cleanup);
    document.addEventListener('keydown', onKey);
  }, 0);
}
