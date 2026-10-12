import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: './tests',
  testMatch: '**/*.spec.ts',
  fullyParallel: true,
  projects: [
    { name: 'chromium', use: { browserName: 'chromium' } },
    { name: 'webkit', use: { browserName: 'webkit' } },
  ],
  use: {
    baseURL: 'http://127.0.0.1:1428',
    userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36',
  },
  webServer: {
    command: 'npm run dev -- --host 127.0.0.1 --port 1428',
    url: 'http://127.0.0.1:1428/tests/fixtures/terminal.html',
    reuseExistingServer: !process.env.CI,
  },
});
