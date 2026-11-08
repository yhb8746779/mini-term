import { test, expect, type Page, type Locator } from '@playwright/test';
import type { SplitNode } from '../src/types';

const tab = (page: Page, id: string) => page.locator(`[data-pane-tab="${id}"]`);
const bar = (page: Page, id: string) => tab(page, id).locator('..');
const content = (page: Page, id: string) => bar(page, id).locator('..').locator(':scope > div').nth(1);
const layout = (page: Page): Promise<SplitNode> => page.evaluate(() => (window as any).paneDragTest.layout());
const ids = (node: SplitNode): unknown => node.type === 'leaf'
  ? node.panes.map(pane => pane.id) : node.children.map(ids);

// One DataTransfer is retained through the whole HTML5 drag, including across
// React remounts. Dispatch inside xterm so TerminalInstance's capture handlers run.
async function start(page: Page, id: string) {
  const data = await page.evaluateHandle(() => new DataTransfer());
  await tab(page, id).dispatchEvent('dragstart', { dataTransfer: data });
  return data;
}
async function over(target: Locator, data: Awaited<ReturnType<typeof start>>, x = 0.5, y = 0.5) {
  const rect = (await target.boundingBox())!;
  const point = { clientX: rect.x + rect.width * x, clientY: rect.y + rect.height * y, dataTransfer: data };
  await target.dispatchEvent('dragover', point);
  return point;
}
async function finish(page: Page, target: Locator, point: Awaited<ReturnType<typeof over>>) {
  await target.dispatchEvent('drop', point);
  // A moved source tab can be unmounted; the browser still ends the drag globally.
  await page.evaluate(() => window.dispatchEvent(new DragEvent('dragend', { bubbles: true })));
  await point.dataTransfer.dispose();
}
async function drag(page: Page, id: string, target: Locator, x = 0.5, y = 0.5) {
  const data = await start(page, id);
  await finish(page, target, await over(target, data, x, y));
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const native = { calls: [] as { cmd: string; args: any }[], writes: [] as unknown[] };
    (window as any).native = native;
    (window as any).__TAURI_INTERNALS__ = {
      transformCallback: () => 1,
      unregisterCallback: () => {},
      invoke: async (cmd: string, args: any = {}) => {
        native.calls.push({ cmd, args });
        if (cmd === 'write_pty') native.writes.push(args);
        if (cmd === 'get_pty_transport') return 'local';
        return undefined;
      },
    };
    localStorage.setItem('mini-term-disable-webgl', '1');
  });
  await page.goto('/tests/fixtures/pane-drag.html');
  await expect(page.locator('[data-pane-tab]')).toHaveCount(4);
  await expect(content(page, 'A').locator('.xterm-screen').first()).toBeVisible();
  await expect(content(page, 'D').locator('.xterm-screen')).toBeVisible();
});

test.afterEach(async ({ page }) => {
  const calls = await page.evaluate(() => (window as any).native.calls);
  expect(calls.filter((call: any) => ['create_pty', 'kill_pty', 'write_pty'].includes(call.cmd))).toEqual([]);
});

test('a: reorder C before A within the left group', async ({ page }) => {
  await drag(page, 'C', tab(page, 'A'), 0.05);
  await expect.poll(async () => ids(await layout(page))).toEqual([['C', 'A', 'B'], ['D']]);
  expect(await bar(page, 'C').locator('[data-pane-tab]').evaluateAll(tabs => tabs.map(t => t.getAttribute('data-pane-tab')))).toEqual(['C', 'A', 'B']);
});

test('b: moving the only right pane into the left bar collapses the root', async ({ page }) => {
  await drag(page, 'D', tab(page, 'C'), 0.95);
  await expect.poll(async () => ids(await layout(page))).toEqual(['A', 'B', 'C', 'D']);
  expect((await layout(page)).type).toBe('leaf');
  await expect.poll(() => page.evaluate(() => (window as any).native.calls.filter((c: any) => c.cmd === 'save_config').length)).toBeGreaterThan(0);
});

test('c: dropping A at the bottom of the right terminal creates a vertical split', async ({ page }) => {
  const target = content(page, 'D').locator('.xterm-screen');
  await drag(page, 'A', target, 0.5, 0.98);
  await expect.poll(async () => ids(await layout(page))).toEqual([['B', 'C'], [['D'], ['A']]]);
  const root = await layout(page);
  expect(root.type).toBe('split');
  if (root.type !== 'split') throw new Error('Expected root split');
  expect(root.direction).toBe('horizontal');
  expect(root.children[1]).toMatchObject({ type: 'split', direction: 'vertical' });
});

test('d: center overlay accepts panes without file overlay or PTY writes', async ({ page }) => {
  const data = await start(page, 'A');
  expect(await data.evaluate(dt => Array.from(dt.types))).toEqual(['application/x-mini-term-pane']);
  const target = content(page, 'D').locator('.xterm-screen');
  const point = await over(target, data);
  await expect(page.getByText('移入此组', { exact: true })).toBeVisible();
  await expect(page.getByText('释放以分屏', { exact: true })).toHaveCount(0);
  await expect(page.getByText('释放以插入路径', { exact: true })).toHaveCount(0);
  await finish(page, target, point);
  await expect.poll(async () => ids(await layout(page))).toEqual([['B', 'C'], ['D', 'A']]);
  await expect(page.getByText('移入此组', { exact: true })).toHaveCount(0);
  await expect(page.getByText('释放以插入路径', { exact: true })).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).native.writes)).toEqual([]);
});

test('e: dropping a single pane at its own edge leaves the layout unchanged', async ({ page }) => {
  const before = await layout(page);
  await drag(page, 'D', content(page, 'D').locator('.xterm-screen'), 0.5, 0.98);
  expect(await layout(page)).toEqual(before);
});
