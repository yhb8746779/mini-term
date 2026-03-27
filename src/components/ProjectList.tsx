import { useCallback } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { invoke } from '@tauri-apps/api/core';
import { useAppStore, genId } from '../store';
import { StatusDot } from './StatusDot';
import type { PaneStatus } from '../types';

export function ProjectList() {
  const config = useAppStore((s) => s.config);
  const activeProjectId = useAppStore((s) => s.activeProjectId);
  const projectStates = useAppStore((s) => s.projectStates);
  const setActiveProject = useAppStore((s) => s.setActiveProject);
  const addProject = useAppStore((s) => s.addProject);
  const removeProject = useAppStore((s) => s.removeProject);

  const handleAddProject = useCallback(async () => {
    const selected = await open({ directory: true, multiple: false });
    if (!selected) return;

    const path = selected as string;
    const name = path.split(/[/\\]/).pop() || path;
    const id = genId();

    addProject({ id, name, path });

    // 持久化配置
    const latestConfig = useAppStore.getState().config;
    invoke('save_config', { config: latestConfig });
  }, [addProject]);

  const handleRemoveProject = useCallback(
    (e: React.MouseEvent, id: string) => {
      e.stopPropagation();
      removeProject(id);
      const latestConfig = useAppStore.getState().config;
      invoke('save_config', { config: latestConfig });
    },
    [removeProject]
  );

  // 获取项目的 AI 聚合状态
  const getProjectAiStatus = (projectId: string): PaneStatus | null => {
    const ps = projectStates.get(projectId);
    if (!ps) return null;
    for (const tab of ps.tabs) {
      if (tab.status === 'ai-working') return 'ai-working';
    }
    return null;
  };

  return (
    <div className="h-full bg-[#12121f] flex flex-col overflow-y-auto">
      <div className="px-2 pt-2 pb-1 text-[10px] text-gray-600 uppercase tracking-widest">
        项目
      </div>

      <div className="flex-1 px-1">
        {config.projects.map((project) => {
          const isActive = project.id === activeProjectId;
          const aiStatus = getProjectAiStatus(project.id);

          return (
            <div
              key={project.id}
              className={`flex items-center gap-1.5 px-2 py-1.5 rounded cursor-pointer text-xs mb-0.5 group ${
                isActive
                  ? 'bg-[#7c83ff22] text-[#7c83ff]'
                  : 'text-gray-500 hover:text-gray-300 hover:bg-[#ffffff08]'
              }`}
              onClick={() => setActiveProject(project.id)}
              title={project.path}
            >
              <span className="truncate flex-1">📁 {project.name}</span>
              {aiStatus && <StatusDot status={aiStatus} />}
              <span
                className="text-gray-700 hover:text-red-400 hidden group-hover:inline"
                onClick={(e) => handleRemoveProject(e, project.id)}
              >
                ✕
              </span>
            </div>
          );
        })}
      </div>

      <div
        className="mx-2 mb-2 px-2 py-1.5 border border-dashed border-gray-700 rounded text-center text-[11px] text-gray-600 cursor-pointer hover:border-[#7c83ff] hover:text-[#7c83ff]"
        onClick={handleAddProject}
      >
        + 添加项目
      </div>
    </div>
  );
}
