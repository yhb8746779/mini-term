import { invoke } from '@tauri-apps/api/core';
import { showErrorToast } from './errorToast';

interface CreatePtyArgs {
  shell: string;
  args: string[];
  cwd: string;
  shellName?: string;
}

/**
 * 统一封装 create_pty，spawn 失败时弹错误 toast 并返回 null。
 *
 * 历史教训：四处调用点（新建 tab / 分屏 / PaneGroup 加 pane / 恢复 layout）原本
 * 直接 await invoke，没有 try/catch。当用户选的 shell 在 PATH 里找不到（典型如
 * pwsh 装在 C:\Program Files\PowerShell\7 但未加 PATH），后端返回 Err，前端 Promise
 * reject 被静默吞掉，UI 表现是"点了新建终端完全没反应"，必须看 devtools 才能发现。
 */
export async function createPtySafe(args: CreatePtyArgs): Promise<number | null> {
  try {
    return await invoke<number>('create_pty', {
      shell: args.shell,
      args: args.args,
      cwd: args.cwd,
    });
  } catch (err) {
    const label = args.shellName ?? args.shell;
    const detail = typeof err === 'string' ? err : err instanceof Error ? err.message : String(err);
    showErrorToast(`无法启动终端「${label}」：${detail}`);
    console.error('[create_pty] failed', { shell: args.shell, cwd: args.cwd, err });
    return null;
  }
}
