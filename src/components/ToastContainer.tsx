import { useAppStore } from '../store';

export function ToastContainer() {
  const notifications = useAppStore((s) => s.notifications);
  const dismissNotification = useAppStore((s) => s.dismissNotification);
  const setActiveProject = useAppStore((s) => s.setActiveProject);

  // 最多同时渲染 5 个，超出排队
  const visible = notifications.slice(0, 5);

  if (visible.length === 0) return null;

  return (
    <div className="toast-stack">
      {visible.map((n) => (
        <div
          key={n.id}
          className="toast-card"
          onClick={() => {
            setActiveProject(n.projectId);
            dismissNotification(n.id);
          }}
        >
          <div className="toast-icon">✓</div>
          <div className="toast-body">
            <div className="toast-name">{n.projectName}</div>
            <div className="toast-desc">AI 已完成 · 点击查看</div>
          </div>
          <div
            className="toast-close"
            onClick={(e) => {
              e.stopPropagation();
              dismissNotification(n.id);
            }}
          >×</div>
        </div>
      ))}
    </div>
  );
}
