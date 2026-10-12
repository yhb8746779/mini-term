import { useAppStore } from '../store';
import { updateSshImageBridges } from '../utils/sshImageBridges';

export function SshImageBridgeSelect({ compact = false }: { compact?: boolean }) {
  const config = useAppStore((state) => state.config);
  const bridges = config.sshImageBridges ?? [];
  const activeId = bridges.some((bridge) => bridge.id === config.activeSshImageBridgeId)
    ? config.activeSshImageBridgeId : '';

  return (
    <label className="flex items-center gap-2 text-[var(--text-muted)] min-w-0">
      <span className="whitespace-nowrap">{compact ? '图片桥接' : '当前使用'}</span>
      <select
        aria-label="当前 SSH 图片桥接配置"
        className={`${compact ? 'max-w-48 text-xs' : 'flex-1 text-base'} min-w-0 bg-[var(--bg-base)] text-[var(--text-primary)] border border-[var(--border-default)] rounded-[var(--radius-sm)] px-2 py-1 outline-none focus:border-[var(--accent)]`}
        value={activeId ?? ''}
        onChange={(event) => void updateSshImageBridges(bridges, event.target.value)}
      >
        <option value="">自动匹配 SSH 目标</option>
        {bridges.map((bridge) => <option key={bridge.id} value={bridge.id}>{bridge.name}</option>)}
      </select>
    </label>
  );
}
