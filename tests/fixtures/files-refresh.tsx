import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { ProjectList } from '../../src/components/ProjectList';
import { FileTree } from '../../src/components/FileTree';
import { useAppStore } from '../../src/store';
import '../../src/styles.css';
import 'allotment/dist/style.css';

// Only native IPC is mocked. No real projects, files, PTYs or clipboard are used.
const reads = new Map<string, number>();
let failPath = '';
let delayPath = '';
let finishDelayed: (() => void) | undefined;
(window as any).__TAURI_INTERNALS__ = {
  transformCallback: () => 1, unregisterCallback: () => {},
  invoke: async (cmd: string, args: any = {}) => {
    if (cmd === 'get_git_status') return [];
    if (cmd === 'get_ai_sessions') return { sessions: [], scanning: false };
    if (cmd !== 'list_directory') return;
    reads.set(args.path, (reads.get(args.path) ?? 0) + 1);
    if (args.path === failPath) throw new Error('fixture directory unavailable');
    if (args.path === delayPath) await new Promise<void>((resolve) => { finishDelayed = resolve; });
    return [{ name: args.path.split('/').pop() + '.txt', path: args.path + '/file.txt', isDir: false, ignored: false }];
  },
};
const projects = ['alpha', 'beta', 'gamma'].map((id) => ({ id, name: id, path: '/fixture/' + id }));
useAppStore.setState({
  activeProjectId: 'alpha', config: { ...useAppStore.getState().config, projects, projectTree: projects.map((p) => p.id) },
  projectStates: new Map(projects.map((p) => [p.id, { id: p.id, tabs: [], activeTabId: '' }])),
});
const tick = () => new Promise((resolve) => setTimeout(resolve, 25));
async function until(check: () => boolean) {
  for (let i = 0; i < 100; i++) { if (check()) return; await tick(); }
  throw new Error('Timed out waiting for UI');
}
function click(selector: string) {
  const element = document.querySelector<HTMLElement>(selector);
  if (!element) throw new Error('Missing element: ' + selector);
  element.click();
}
const fileText = () => document.querySelector('#files')?.textContent ?? '';
const row = (id: string) => `div[draggable="true"][title="/fixture/${id}"]`;
function Fixture() {
  const id = useAppStore((s) => s.activeProjectId);
  const revision = useAppStore((s) => s.projectSelectionRevision);
  const [result, setResult] = useState('尚未运行');
  async function run() {
    const passed: string[] = [];
    try {
      await until(() => fileText().includes('alpha.txt'));
      const before = reads.get('/fixture/alpha')!;
      click(row('alpha'));
      await until(() => reads.get('/fixture/alpha')! > before);
      passed.push('重复点击当前项目会重新读取');
      click('button[title="搜索项目 (Ctrl+F)"]');
      await until(() => !!document.querySelector('input[placeholder="搜索项目名称或路径…"]'));
      const input = document.querySelector<HTMLInputElement>('input[placeholder="搜索项目名称或路径…"]')!;
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, 'beta');
      input.dispatchEvent(new Event('input', { bubbles: true }));
      await until(() => !!document.querySelector('button[title="/fixture/beta"]'));
      click('button[title="/fixture/beta"]');
      await until(() => fileText().includes('beta.txt'));
      passed.push('点击搜索结果路径可以切换并读取 Files');
      const betaReads = reads.get('/fixture/beta')!;
      click(row('beta'));
      await until(() => reads.get('/fixture/beta')! > betaReads);
      passed.push('搜索中重复点击项目名称可以刷新');
      // Return to a previously visited project while disk IO is deliberately held.
      delayPath = '/fixture/alpha';
      useAppStore.getState().setActiveProject('alpha');
      await until(() => !!finishDelayed && fileText().includes('alpha.txt'));
      passed.push('组件重建后缓存仍可显示');
      useAppStore.getState().setActiveProject('beta');
      await until(() => fileText().includes('beta.txt'));
      finishDelayed!(); delayPath = '';
      await tick();
      if (fileText().includes('alpha.txt')) throw new Error('Late response overwrote current project');
      passed.push('旧项目延迟响应不会覆盖当前项目');
      failPath = '/fixture/gamma';
      useAppStore.getState().setActiveProject('gamma');
      await until(() => fileText().includes('目录读取失败'));
      failPath = '';
      click('#files button.mt-1');
      await until(() => fileText().includes('gamma.txt') && !fileText().includes('目录读取失败'));
      passed.push('读取失败可见并能重试恢复');
      setResult('PASS · ' + passed.length + '\n' + passed.join('\n'));
    } catch (error) { setResult('FAIL · ' + String(error) + '\n' + passed.join('\n')); }
  }
  return <><button id="run" onClick={run}>运行 Files 回归测试</button><pre id="result">{result}</pre>
    <div style={{ display: 'flex', height: 550 }}><div style={{ width: 280 }}><ProjectList /></div>
      <div id="files" style={{ width: 400 }}><FileTree key={`${id}:${revision}`} /></div></div></>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
