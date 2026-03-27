import { useEffect, useCallback } from 'react';
import { Allotment } from 'allotment';
import { invoke } from '@tauri-apps/api/core';
import { useAppStore } from './store';
import { TerminalArea } from './components/TerminalArea';
import { ProjectList } from './components/ProjectList';
import { useTauriEvent } from './hooks/useTauriEvent';
import type { AppConfig, PtyStatusChangePayload, PtyExitPayload, PaneStatus } from './types';

export function App() {
  const aiPanelVisible = useAppStore((s) => s.aiPanelVisible);
  const activeProjectId = useAppStore((s) => s.activeProjectId);
  const config = useAppStore((s) => s.config);
  const setConfig = useAppStore((s) => s.setConfig);
  const updatePaneStatusByPty = useAppStore((s) => s.updatePaneStatusByPty);

  useEffect(() => {
    invoke<AppConfig>('load_config').then((cfg) => {
      setConfig(cfg);
      // 仅初始化 projectState（不调用 addProject 避免重复）
      const { projectStates } = useAppStore.getState();
      const newStates = new Map(projectStates);
      for (const p of cfg.projects) {
        if (!newStates.has(p.id)) {
          newStates.set(p.id, { id: p.id, tabs: [], activeTabId: '' });
        }
      }
      useAppStore.setState({
        projectStates: newStates,
        activeProjectId: cfg.projects[0]?.id ?? null,
      });
    });
  }, []);

  useTauriEvent<PtyStatusChangePayload>('pty-status-change', useCallback((payload) => {
    updatePaneStatusByPty(payload.ptyId, payload.status as PaneStatus);
  }, [updatePaneStatusByPty]));

  useTauriEvent<PtyExitPayload>('pty-exit', useCallback((payload) => {
    if (payload.exitCode !== 0) {
      updatePaneStatusByPty(payload.ptyId, 'error');
    }
  }, [updatePaneStatusByPty]));

  return (
    <div className="flex flex-col h-full">
      {/* 顶部工具栏 */}
      <div className="flex items-center gap-3 px-3 py-1.5 bg-[#1a1a2e] border-b border-[#333] text-xs text-[#a0a0c0]">
        <span className="font-bold text-[#7c83ff]">Mini-Term</span>
        <span className="opacity-40">|</span>
        <span className="cursor-pointer hover:text-white">终端</span>
        <span className="cursor-pointer hover:text-white">设置</span>
      </div>

      {/* 主体三栏 */}
      <div className="flex-1 overflow-hidden">
        <Allotment>
          {/* 左栏：项目列表 */}
          <Allotment.Pane preferredSize={200} minSize={140} maxSize={350}>
            <ProjectList />
          </Allotment.Pane>

          {/* 中栏：文件树 */}
          <Allotment.Pane preferredSize={280} minSize={180}>
            <div className="h-full bg-[#16162a] p-2 text-xs text-gray-400">
              文件树占位
            </div>
          </Allotment.Pane>

          {/* 右栏：终端 + AI 历史 */}
          <Allotment.Pane>
            <Allotment>
              <Allotment.Pane>
                {(() => {
                  const activeProject = config.projects.find((p) => p.id === activeProjectId);
                  return activeProject ? (
                    <TerminalArea projectId={activeProject.id} projectPath={activeProject.path} />
                  ) : (
                    <div className="h-full bg-[#0d0d1a] flex items-center justify-center text-gray-500 text-sm">
                      请先在左栏添加项目
                    </div>
                  );
                })()}
              </Allotment.Pane>

              {aiPanelVisible && (
                <Allotment.Pane preferredSize={180} minSize={140} maxSize={280} snap>
                  <div className="h-full bg-[#12121f] border-l-2 border-[#7c83ff33] p-2 text-xs text-gray-400">
                    AI 历史占位
                  </div>
                </Allotment.Pane>
              )}
            </Allotment>
          </Allotment.Pane>
        </Allotment>
      </div>
    </div>
  );
}
