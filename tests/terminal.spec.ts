import { test, expect, type Page } from '@playwright/test';

async function boot(page: Page, options: Record<string, unknown> = {}) {
  await page.addInitScript((opts) => {
    const state = Object.assign({
      transport: 'local', kind: 'image', text: 'clipboard text', paths: [],
      bridge: true, shared: true, upload: true, copyFails: false,
      calls: [], writes: [], copies: [],
    }, opts);
    (window as any).native = state;
    (window as any).__TAURI_INTERNALS__ = {
      transformCallback: () => 1,
      unregisterCallback: () => {},
      invoke: async (cmd: string, args: any = {}) => {
        state.calls.push({ cmd, args });
        switch (cmd) {
          case 'get_pty_transport': return state.transport;
          case 'read_clipboard_file_paths_macos':
          case 'read_clipboard_file_paths': return state.paths;
          case 'plugin:clipboard-manager|read_image': return 7;
          case 'plugin:image|size': return { width: 1, height: 1 };
          case 'plugin:image|rgba': return [255, 0, 0, 255];
          case 'save_clipboard_rgba_image_for_path_paste':
            return { savedPath: '/cache/image.png', terminalPath: '/cache/image.png' };
          case 'copy_image_file_for_path_paste':
            return { savedPath: args.path, terminalPath: '/cache/image.png' };
          case 'probe_ssh_clipboard_bridge':
            if (!state.bridge) throw new Error('No bridge');
            return;
          case 'stage_image_in_ssh_shared_directory':
            if (!state.shared) throw new Error('No shared directory');
            return '/workspace/image.png';
          case 'upload_image_over_ssh':
            if (!state.upload) throw new Error('Upload failed');
            return '/tmp/image.png';
          case 'write_pty': state.writes.push(args.data); return;
          case 'plugin:clipboard-manager|write_text':
            if (state.copyFails) throw new Error('Clipboard unavailable');
            state.copies.push(args.text); return;
          case 'plugin:clipboard-manager|read_text': return state.text;
          default: return 1;
        }
      },
    };
    Object.defineProperty(navigator, 'clipboard', { value: {
      read: async () => state.kind === 'file' ? [] : [{
        types: state.kind === 'image' ? ['image/png']
          : state.kind === 'wechat' ? ['application/x-wechat'] : ['text/plain'],
        getType: async () => new Blob([state.text]),
      }],
      writeText: async (text: string) => {
        if (state.copyFails) throw new Error('Clipboard unavailable');
        state.copies.push(text);
      },
    } });
    localStorage.setItem('mini-term-disable-webgl', '1');
  }, options);
  await page.goto('/tests/fixtures/terminal.html');
  await page.waitForFunction(() => (window as any).terminalTest?.term());
  await page.evaluate(async () => {
    const term = (window as any).terminalTest.term();
    await new Promise<void>(resolve => term.write('Selection diagnostic\r\n\x1b[?2004h', resolve));
  });
}

async function paste(page: Page, provider = 'codex') {
  await page.evaluate(async (p) => {
    (window as any).terminalTest.provider(p);
    await (window as any).terminalTest.paste();
  }, provider);
}
const writes = (page: Page) => page.evaluate(() => (window as any).native.writes);
const commands = (page: Page) => page.evaluate(() => (window as any).native.calls.map((c: any) => c.cmd));

test('local macOS Codex pastes screenshot as an image shortcut, not a path prompt', async ({ page }) => {
  await boot(page);
  await paste(page);
  await expect.poll(() => writes(page)).toEqual(['\x1bv']);
  expect(await commands(page)).not.toContain('save_clipboard_rgba_image_for_path_paste');
});

test('local Codex Finder image loads bitmap before image shortcut', async ({ page }) => {
  await boot(page, { kind: 'file', paths: ['/pictures/test.png'] });
  await paste(page);
  await expect.poll(() => writes(page)).toEqual(['\x1bv']);
  const calls = await commands(page);
  expect(calls.indexOf('load_image_to_clipboard')).toBeLessThan(calls.indexOf('write_pty'));
});

test('WeChat image hidden by WebKit is detected through the native clipboard fallback', async ({ page }) => {
  await boot(page, { kind: 'wechat' });
  await paste(page);
  await expect.poll(() => writes(page)).toEqual(['\x1bv']);
  expect(await commands(page)).toContain('plugin:clipboard-manager|read_image');
});

test('local Claude keeps its existing image-path strategy', async ({ page }) => {
  await boot(page);
  await paste(page, 'claude');
  await expect.poll(() => writes(page)).toEqual(['\x1b[200~请读取图片 @/cache/image.png\x1b[201~']);
});

for (const kind of ['image', 'file']) {
  test(`SSH Claude ${kind}: PNG normalization precedes native bridge shortcut`, async ({ page }) => {
    await boot(page, { transport: 'ssh', kind, paths: kind === 'file' ? ['/pictures/test.png'] : [] });
    await paste(page, 'claude');
    await expect.poll(() => writes(page)).toEqual(['\x16']);
    const calls = await commands(page);
    expect(calls).toContain('probe_ssh_clipboard_bridge');
    expect(calls.indexOf('load_image_to_clipboard')).toBeLessThan(calls.indexOf('write_pty'));
    expect(calls).not.toContain('stage_image_in_ssh_shared_directory');
    expect(calls).not.toContain('upload_image_over_ssh');
  });
}

test('SSH Claude keeps shared-directory fallback when bridge unavailable', async ({ page }) => {
  await boot(page, { transport: 'ssh', bridge: false });
  await paste(page, 'claude');
  await expect.poll(() => writes(page)).toEqual(['\x1b[200~请读取图片 @/workspace/image.png\x1b[201~']);
  expect(await commands(page)).not.toContain('upload_image_over_ssh');
});

test('SSH Claude keeps upload fallback when neither bridge nor shared directory available', async ({ page }) => {
  await boot(page, { transport: 'ssh', bridge: false, shared: false });
  await paste(page, 'claude');
  await expect.poll(() => writes(page)).toEqual(['\x1b[200~请读取图片 @/tmp/image.png\x1b[201~']);
});

test('failed SSH upload never injects an unreadable local path', async ({ page }) => {
  await boot(page, { transport: 'ssh', bridge: false, shared: false, upload: false });
  await paste(page, 'claude');
  expect(await writes(page)).toEqual([]);
});

for (const provider of ['codex', 'claude']) {
  test(`${provider}: plain text remains bracketed paste`, async ({ page }) => {
    await boot(page, { kind: 'text' });
    await paste(page, provider);
    await expect.poll(() => writes(page)).toEqual(['\x1b[200~clipboard text\x1b[201~']);
  });
}

async function selectWithMouseMode(page: Page) {
  const screen = page.locator('.xterm-screen');
  const box = (await screen.boundingBox())!;
  // Move before selection, since ANY-mode motion itself clears xterm selection.
  await page.mouse.move(box.x + 50, box.y + 10);
  await page.evaluate(async () => {
    const term = (window as any).terminalTest.term();
    await new Promise<void>(resolve => term.write('\x1b[?1003h\x1b[?1006h', resolve));
    term.select(0, 0, 9);
    (window as any).native.writes = [];
  });
}

test('mouse-reporting TUI: selected text + right click copies without any PTY input', async ({ page }) => {
  await boot(page);
  await selectWithMouseMode(page);
  await page.mouse.down({ button: 'right' });
  await page.mouse.up({ button: 'right' });
  await expect.poll(() => page.evaluate(() => (window as any).native.copies)).toEqual(['Selection']);
  expect(await writes(page)).toEqual([]);
});

test('right click with no selection pastes exactly once without mouse reports', async ({ page }) => {
  await boot(page, { kind: 'text' });
  await selectWithMouseMode(page);
  await page.evaluate(() => (window as any).terminalTest.term().clearSelection());
  await page.mouse.down({ button: 'right' });
  await page.mouse.up({ button: 'right' });
  await expect.poll(() => writes(page)).toEqual(['\x1b[200~clipboard text\x1b[201~']);
});

test('contextmenu without mousedown uses current selection', async ({ page }) => {
  await boot(page);
  await selectWithMouseMode(page);
  await page.locator('.xterm-screen').dispatchEvent('contextmenu', { button: 2 });
  await expect.poll(() => page.evaluate(() => (window as any).native.copies)).toEqual(['Selection']);
  expect(await writes(page)).toEqual([]);
});

test('macOS Control-click copies selected text without sending a left click to the TUI', async ({ page }) => {
  await boot(page);
  await selectWithMouseMode(page);
  const screen = page.locator('.xterm-screen');
  await screen.dispatchEvent('mousedown', { button: 0, ctrlKey: true });
  await screen.dispatchEvent('contextmenu', { button: 0, ctrlKey: true });
  await screen.dispatchEvent('mouseup', { button: 0, ctrlKey: true });
  await expect.poll(() => page.evaluate(() => (window as any).native.copies)).toEqual(['Selection']);
  expect(await writes(page)).toEqual([]);
});

test('ordinary left clicks still reach a mouse-reporting Claude/Codex TUI', async ({ page }) => {
  await boot(page);
  await selectWithMouseMode(page);
  await page.mouse.down({ button: 'left' });
  await page.mouse.up({ button: 'left' });
  await expect.poll(async () => (await writes(page)).join('')).toMatch(/\x1b\[<0;\d+;\d+M/);
});

test('failed copy retains selection and never falls through to paste', async ({ page }) => {
  await boot(page, { copyFails: true });
  await selectWithMouseMode(page);
  await page.mouse.down({ button: 'right' });
  await page.mouse.up({ button: 'right' });
  await expect.poll(() => commands(page)).toContain('plugin:clipboard-manager|write_text');
  expect(await page.evaluate(() => (window as any).terminalTest.selected())).toBe('Selection');
  expect(await writes(page)).toEqual([]);
});

test('DOM selection outside this terminal cannot be copied as terminal text', async ({ page }) => {
  await boot(page);
  const selected = await page.evaluate(() => {
    const other = document.createElement('div');
    other.textContent = 'Another pane';
    document.body.append(other);
    const range = document.createRange();
    range.selectNodeContents(other);
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(range);
    return (window as any).terminalTest.selected();
  });
  expect(selected).toBe('');
});

const bridgeProfiles = [
  { id: 'a', name: '沙箱 A', sshHost: 'sandbox', remoteDirectory: '/bridge/a', probeCommand: 'test -x {directory}/bin/xclip', filePrefix: 'clipboard-', hostSharedDirectory: '/host/a', remoteSharedDirectory: '/remote/a', sharedImageSubdirectory: 'images' },
  { id: 'b', name: '沙箱 B', sshHost: 'sandbox', remoteDirectory: '/bridge/b', probeCommand: 'test -x {directory}/bin/xclip', filePrefix: 'clipboard-', hostSharedDirectory: '/host/b', remoteSharedDirectory: '/remote/b', sharedImageSubdirectory: 'images' },
];

test('selected bridge is used by both SSH probing and shared-directory fallback', async ({ page }) => {
  await boot(page, { transport: 'ssh', bridge: false });
  await page.evaluate((profiles) => (window as any).terminalTest.setBridges(profiles, 'b'), bridgeProfiles);
  await paste(page, 'claude');
  const routed = await page.evaluate(() => (window as any).native.calls.filter((call: any) =>
    ['probe_ssh_clipboard_bridge', 'stage_image_in_ssh_shared_directory'].includes(call.cmd)));
  expect(routed).toHaveLength(2);
  for (const call of routed) expect(call.args.bridges).toEqual([bridgeProfiles[1]]);
});

test('automatic matching retains all existing profiles', async ({ page }) => {
  await boot(page, { transport: 'ssh' });
  await page.evaluate((profiles) => (window as any).terminalTest.setBridges(profiles), bridgeProfiles);
  await paste(page, 'claude');
  const call = await page.evaluate(() => (window as any).native.calls.find((call: any) => call.cmd === 'probe_ssh_clipboard_bridge'));
  expect(call.args.bridges).toEqual(bridgeProfiles);
});

test('bridge profile copying, independent editing, quick switching and deletion', async ({ page }) => {
  await boot(page);
  await page.evaluate((profiles) => {
    (window as any).terminalTest.setBridges(profiles.slice(0, 1), 'a');
    (window as any).terminalTest.showBridgeSettings();
  }, bridgeProfiles);
  await page.getByRole('button', { name: '复制配置', exact: true }).click();
  const profiles = await page.evaluate(() => (window as any).terminalTest.config().sshImageBridges);
  expect(profiles).toHaveLength(2);
  expect(profiles[1]).toEqual({ ...profiles[0], id: expect.any(String), name: '沙箱 A 副本' });
  expect(profiles[1].id).not.toBe('a');
  expect(await page.evaluate(() => (window as any).terminalTest.config().activeSshImageBridgeId)).toBe('a');
  await page.getByRole('button', { name: '编辑', exact: true }).nth(1).click();
  await page.getByPlaceholder('远端图片桥接目录', { exact: true }).fill('/bridge/copy');
  await page.getByRole('button', { name: '保存', exact: true }).click();
  const edited = await page.evaluate(() => (window as any).terminalTest.config().sshImageBridges);
  expect(edited[0].remoteDirectory).toBe('/bridge/a');
  expect(edited[1].remoteDirectory).toBe('/bridge/copy');
  // The first selector is the same compact component used in the main toolbar.
  await page.getByRole('combobox').first().selectOption(profiles[1].id);
  await expect(page.getByRole('combobox').nth(1)).toHaveValue(profiles[1].id);
  await expect.poll(() => page.evaluate(() => (window as any).native.calls
    .filter((call: any) => call.cmd === 'save_config').at(-1)?.args.config.activeSshImageBridgeId)).toBe(profiles[1].id);
  await page.getByRole('button', { name: '删除', exact: true }).nth(1).click();
  await expect(page.getByRole('combobox').first()).toHaveValue('');
  expect(await page.evaluate(() => (window as any).terminalTest.config().sshImageBridges)).toHaveLength(1);
});

test.describe('Windows regression', () => {
  test.use({ userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36' });

  for (const mouseMode of [false, true]) {
    test(`selected text right-click copies only, mouse reporting=${mouseMode}`, async ({ page }) => {
      await boot(page, { kind: 'text' });
      await selectWithMouseMode(page);
      if (!mouseMode) await page.evaluate(async () => {
        await new Promise<void>(resolve => (window as any).terminalTest.term().write('\x1b[?1003l', resolve));
      });
      await page.mouse.down({ button: 'right' });
      await page.mouse.up({ button: 'right' });
      await expect.poll(() => page.evaluate(() => (window as any).native.copies)).toEqual(['Selection']);
      expect(await writes(page)).toEqual([]);
    });

    test(`unselected right-click pastes once, mouse reporting=${mouseMode}`, async ({ page }) => {
      await boot(page, { kind: 'text' });
      await selectWithMouseMode(page);
      await page.evaluate(async (enabled) => {
        const term = (window as any).terminalTest.term();
        term.clearSelection();
        if (!enabled) await new Promise<void>(resolve => term.write('\x1b[?1003l', resolve));
      }, mouseMode);
      await page.mouse.down({ button: 'right' });
      await page.mouse.up({ button: 'right' });
      await expect.poll(() => writes(page)).toEqual(['\x1b[200~clipboard text\x1b[201~']);
    });
  }

  for (const provider of ['codex', 'claude']) {
    test(`${provider} local screenshot keeps Alt+V`, async ({ page }) => {
      await boot(page);
      await paste(page, provider);
      await expect.poll(() => writes(page)).toEqual(['\x1bv']);
      expect(await commands(page)).not.toContain('save_clipboard_rgba_image_for_path_paste');
    });

    test(`${provider} SSH screenshot keeps native bridge Alt+V`, async ({ page }) => {
      await boot(page, { transport: 'ssh' });
      await paste(page, provider);
      await expect.poll(() => writes(page)).toEqual(['\x1bv']);
      expect(await commands(page)).toContain('probe_ssh_clipboard_bridge');
      expect(await commands(page)).not.toContain('upload_image_over_ssh');
    });
  }

  test('copy failure retains selection without pasting', async ({ page }) => {
    await boot(page, { copyFails: true });
    await selectWithMouseMode(page);
    await page.mouse.down({ button: 'right' });
    await page.mouse.up({ button: 'right' });
    await expect.poll(() => commands(page)).toContain('plugin:clipboard-manager|write_text');
    expect(await page.evaluate(() => (window as any).terminalTest.selected())).toBe('Selection');
    expect(await writes(page)).toEqual([]);
  });

  test('Control-left click is still sent to the TUI on Windows', async ({ page }) => {
    await boot(page);
    await selectWithMouseMode(page);
    await page.locator('.xterm-screen').dispatchEvent('mousedown', { button: 0, ctrlKey: true, clientX: 60, clientY: 30 });
    await expect.poll(async () => (await writes(page)).join('')).toMatch(/\x1b\[<16;\d+;\d+M/);
    expect(await page.evaluate(() => (window as any).native.copies)).toEqual([]);
  });
});
