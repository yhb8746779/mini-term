import { persistConfig, useAppStore } from '../store';
import type { AppConfig, SshImageBridgeConfig } from '../types';
import { showErrorToast } from './errorToast';

/** 选中的配置仍须由后端校验 SSH 目标，不能把图片投到无关沙箱。 */
export function selectedSshImageBridges(config: AppConfig): SshImageBridgeConfig[] {
  const bridges = config.sshImageBridges ?? [];
  const selected = bridges.find((bridge) => bridge.id === config.activeSshImageBridgeId);
  return selected ? [selected] : bridges;
}

let saveQueue = Promise.resolve();

export function updateSshImageBridges(
  bridges: SshImageBridgeConfig[],
  activeId = useAppStore.getState().config.activeSshImageBridgeId,
): Promise<void> {
  const { config, setConfig } = useAppStore.getState();
  setConfig({
    ...config,
    sshImageBridges: bridges,
    activeSshImageBridgeId: bridges.some((bridge) => bridge.id === activeId) ? activeId : undefined,
  });
  // 快速连续切换时按顺序保存，避免旧请求晚到而覆盖新选择。
  saveQueue = saveQueue.then(async () => { await persistConfig(); }).catch((error) => {
    showErrorToast(`保存图片桥接配置失败：${String(error)}`);
  });
  return saveQueue;
}
