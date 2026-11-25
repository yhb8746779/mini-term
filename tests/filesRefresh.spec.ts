import { test, expect } from '@playwright/test';

// Fixture exercises real project selection and FileTree, mocking only native IPC.
test('Files refreshes from search results and recovers after failed or delayed reads', async ({ page }) => {
  await page.goto('/tests/fixtures/files-refresh.html');
  await page.locator('#run').click();
  await expect(page.locator('#result')).toContainText('PASS · 6', { timeout: 15000 });
  await expect(page.locator('#files')).toContainText('gamma.txt');
  await expect(page.locator('#files [role="alert"]')).toHaveCount(0);
});
