import { useEffect, useRef } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export function useTauriEvent<T>(event: string, handler: (payload: T) => void) {
  const handlerRef = useRef(handler);
  handlerRef.current = handler;

  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;
    listen<T>(event, (e) => handlerRef.current(e.payload)).then((fn) => {
      // 如果在 Promise resolve 之前组件已卸载,立刻调用返回的 unlisten,
      // 否则保存下来供后续 cleanup 使用
      if (cancelled) {
        fn();
      } else {
        unlisten = fn;
      }
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [event]);
}
