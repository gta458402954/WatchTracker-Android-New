import { expect, test, type Locator, type Page } from '@playwright/test';
import { setupMockIpc } from './fixtures/mockIpc';

const focusableSelector = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled]):not([type="hidden"])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

async function focusBoundary(dialog: Locator, boundary: 'first' | 'last') {
  await dialog.evaluate((element, { selector, boundary: edge }) => {
    const candidates = Array.from(element.querySelectorAll<HTMLElement>(selector))
      .filter(candidate => candidate.getClientRects().length > 0);
    const target = edge === 'first' ? candidates[0] : candidates[candidates.length - 1];
    target?.focus();
  }, { selector: focusableSelector, boundary });
}

async function activeElementIsBoundary(dialog: Locator, boundary: 'first' | 'last') {
  return dialog.evaluate((element, { selector, boundary: edge }) => {
    const candidates = Array.from(element.querySelectorAll<HTMLElement>(selector))
      .filter(candidate => candidate.getClientRects().length > 0);
    const target = edge === 'first' ? candidates[0] : candidates[candidates.length - 1];
    return document.activeElement === target;
  }, { selector: focusableSelector, boundary });
}

async function expectFocusRestored(page: Page, trigger: Locator) {
  await expect(trigger).toBeFocused();
  await expect.poll(() => page.locator('body').evaluate(body => body.style.overflow)).toBe('');
}

test('@dialog-accessibility traps focus and restores each launcher', async ({ page }) => {
  await setupMockIpc(page);
  await page.goto('/');

  const settingsTrigger = page.getByRole('button', { name: '设置' });
  await settingsTrigger.click();
  const settingsDialog = page.getByRole('dialog', { name: /设置中心/ });
  await expect(settingsDialog).toBeVisible();
  await expect(settingsDialog.getByRole('button', { name: /基础配置/ })).toBeFocused();
  await expect.poll(() => page.locator('body').evaluate(body => body.style.overflow)).toBe('hidden');
  await focusBoundary(settingsDialog, 'first');
  await page.keyboard.press('Shift+Tab');
  await expect.poll(() => activeElementIsBoundary(settingsDialog, 'last')).toBe(true);
  await focusBoundary(settingsDialog, 'last');
  await page.keyboard.press('Tab');
  await expect.poll(() => activeElementIsBoundary(settingsDialog, 'first')).toBe(true);
  await page.keyboard.press('Escape');
  await expect(settingsDialog).toHaveCount(0);
  await expectFocusRestored(page, settingsTrigger);

  const moreTrigger = page.getByRole('button', { name: '更多操作' });
  await moreTrigger.click();
  await page.getByRole('menuitem', { name: /添加记录/ }).click();
  const recordDialog = page.getByRole('dialog', { name: '添加新记录' });
  await expect(recordDialog).toBeVisible();
  await expect(page.getByPlaceholder('请输入中文名称')).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(recordDialog).toHaveCount(0);
  await expectFocusRestored(page, moreTrigger);

  await moreTrigger.click();
  await page.getByRole('menuitem', { name: '数据看板' }).click();
  const dashboardDialog = page.getByRole('dialog', { name: '观看概览' });
  await expect(dashboardDialog).toBeVisible();
  await expect(dashboardDialog.getByRole('button', { name: '关闭观看概览' })).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(dashboardDialog).toHaveCount(0);
  await expectFocusRestored(page, moreTrigger);
});
