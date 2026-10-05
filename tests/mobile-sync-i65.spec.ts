import { expect, test } from '@playwright/test';
import { mockSnapshot, setupMockIpc } from './fixtures/mockIpc';

test.use({ userAgent: 'Mozilla/5.0 (Linux; Android 16; Pixel) AppleWebKit/537.36 Chrome/140 Mobile' });
const configured = { webdav_creds: 'androidkeystore:v1', webdav_url: 'http://127.0.0.1:18144/dav/' };

for (const status of ['success', 'pending', 'readOnlyFrozen']) {
  test(`I6.5 manual facade respects Rust ${status} and never falls through to S1`, async ({ page }) => {
    await setupMockIpc(page, { settings: configured, s2CycleStatus: status });
    await page.goto('/');
    const result = await page.evaluate(async () => (await import('/src/shared/lib/webdav.ts')).syncToWebDAV());
    expect(result.s2Managed).toBe(true);
    expect(result.ok).toBe(status === 'success');
    const snapshot = await mockSnapshot(page);
    const call = snapshot.calls.find(call => call.command === 's2_sync_cycle');
    expect(call?.args.input).toMatchObject({ automatic: false, targetEpoch: 1 });
    expect(snapshot.calls.some(call => ['webdav_request', 'commit_sync_result', 'record_sync_failure'].includes(call.command))).toBe(false);
  });
}

test('I6.5 startup, Android resume and online signal the same Rust command', async ({ page }) => {
  await setupMockIpc(page, { settings: configured, s2CycleStatus: 'success' });
  await page.goto('/');
  await expect.poll(async () => (await mockSnapshot(page)).calls.filter(call => call.command === 's2_sync_cycle').length, { timeout: 10_000 }).toBeGreaterThan(0);
  const before = (await mockSnapshot(page)).calls.filter(call => call.command === 's2_sync_cycle').length;
  await page.evaluate(() => { window.dispatchEvent(new Event('watchtracker:android-resume')); window.dispatchEvent(new Event('online')); });
  await expect.poll(async () => (await mockSnapshot(page)).calls.filter(call => call.command === 's2_sync_cycle').length).toBeGreaterThan(before);
  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.filter(call => call.command === 's2_sync_cycle').every(call => (call.args.input as { automatic: boolean }).automatic)).toBe(true);
  expect(snapshot.calls.some(call => ['webdav_request', 'record_sync_failure'].includes(call.command))).toBe(false);
});

test('I6.5 pending startup keeps Rust backoff without a second TS failure write', async ({ page }) => {
  await setupMockIpc(page, { settings: configured, s2CycleStatus: 'pending' });
  await page.goto('/');
  await expect.poll(async () => (await mockSnapshot(page)).calls.filter(call => call.command === 's2_sync_cycle').length, { timeout: 10_000 }).toBe(1);
  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.some(call => call.command === 'record_sync_failure')).toBe(false);
  const scheduler = JSON.parse(snapshot.settings.sync_scheduler_v1 ?? '{}');
  expect(scheduler.consecutiveFailures).toBe(1);
  expect(scheduler.nextAttemptAt).toBeTruthy();
  expect(scheduler.lastSuccessAt).toBeNull();
});
