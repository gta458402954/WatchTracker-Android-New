import { expect, test, type Page } from '@playwright/test';
import type { WatchRecord } from '../src/shared/types';
import type { SyncPayloadV3 } from '../src/shared/lib/syncMerge';
import { mockSnapshot, setupMockIpc } from './fixtures/mockIpc';

function record(id: string, overrides: Partial<WatchRecord> = {}): WatchRecord {
  return {
    id,
    originalName: `${id} original`,
    chineseName: id,
    progress: '',
    totalEpisodes: null,
    movieProgress: null,
    movieDuration: null,
    releaseYear: '2026',
    posterPath: null,
    status: '未看',
    platform: '',
    rating: null,
    startDate: '',
    endDate: '',
    notes: '',
    createdAt: '2026-01-01T00:00:00.000Z',
    updatedAt: '2026-01-01T00:00:00.000Z',
    imdbId: null,
    mediaType: '电影',
    contentTags: null,
    originCountry: null,
    rev: 1,
    revActor: 'fixture',
    ...overrides,
  };
}

function payload(records: WatchRecord[], overrides: Partial<SyncPayloadV3> = {}): SyncPayloadV3 {
  return {
    schemaVersion: 3,
    documentId: 'sync-reliability-document',
    revision: 1,
    commitId: 'sync-reliability-commit',
    parentCommitId: null,
    writerId: 'remote-device',
    committedAt: '2026-08-02T00:00:00.000Z',
    records,
    tombstones: [],
    ...overrides,
  };
}

const credentials = {
  webdav_creds: 'encrypted:user:password',
  webdav_url: 'https://example.test/watchtracker/',
};

async function webdavCallCount(page: Page): Promise<number> {
  return (await mockSnapshot(page)).calls.filter(call => call.command === 'webdav_request').length;
}

async function runSync(page: Page) {
  return page.evaluate(async () => (await import('/src/shared/lib/webdav.ts')).syncToWebDAV());
}

function cleanConditionalSettings(baseline: SyncPayloadV3, etag = '"conditional-etag"') {
  return {
    ...credentials,
    sync_v3_baseline: JSON.stringify(baseline),
    sync_v3_remote_etag: etag,
    sync_v2_source_fingerprint: '"legacy-1"',
    sync_outbox_v1: JSON.stringify({
      version: 1, pending: false, dirtyGeneration: 0, reasons: [], firstQueuedAt: null, lastQueuedAt: null,
    }),
  };
}

test('@conditional-pull clean unchanged remote uses PROPFIND without v3 GET, commit, or PUT', async ({ page }) => {
  const local = record('conditional-clean');
  const baseline = payload([local]);
  await setupMockIpc(page, {
    records: [local], webdavV3Remote: baseline, webdavV3Etag: '"conditional-etag"',
    webdavConditionalGet: true, settings: cleanConditionalSettings(baseline),
  });
  await page.goto('/');

  expect(await runSync(page)).toMatchObject({ ok: true, records: [expect.objectContaining({ id: local.id })] });

  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.some(call => call.command === 'webdav_request'
    && call.args.method === 'PROPFIND' && String(call.args.url).endsWith('records-v3.json'))).toBe(true);
  expect(snapshot.calls.some(call => call.command === 'webdav_request'
    && call.args.method === 'GET' && String(call.args.url).endsWith('records-v3.json'))).toBe(false);
  expect(snapshot.calls.some(call => call.command === 'record_sync_remote_unchanged')).toBe(true);
  expect(snapshot.calls.some(call => call.command === 'commit_sync_result')).toBe(false);
  expect(snapshot.calls.some(call => call.command === 'webdav_request' && call.args.method === 'PUT')).toBe(false);
  expect(snapshot.calls.some(call => call.command === 'webdav_request'
    && call.args.method === 'GET' && String(call.args.url).endsWith('records.json'))).toBe(true);
  expect(snapshot.recoveryPoints).toHaveLength(0);
  expect(snapshot.records).toEqual([local]);
  expect(snapshot.settings.sync_v3_baseline).toBe(JSON.stringify(baseline));
  expect(snapshot.settings.sync_v3_remote_etag).toBe('"conditional-etag"');
  expect(JSON.parse(snapshot.settings.sync_outbox_v1 as string).pending).toBe(false);
  const scheduler = JSON.parse(snapshot.settings.sync_scheduler_v1 as string);
  expect(scheduler.lastRemoteCheckAt).not.toBeNull();
  expect(scheduler.lastSuccessAt).not.toBeNull();
  expect(scheduler.consecutiveFailures).toBe(0);
});

test('@conditional-pull PROPFIND same detects a concurrent local edit through narrow stale CAS', async ({ page }) => {
  const local = record('propfind-toctou');
  const baseline = payload([local]);
  await setupMockIpc(page, {
    records: [local], webdavV3Remote: baseline, webdavV3Etag: '"propfind-etag"', mutateLocalDuringPropfind: true,
    settings: cleanConditionalSettings(baseline, '"propfind-etag"'),
  });
  await page.goto('/');

  expect(await runSync(page)).toMatchObject({ ok: false, error: 'stale_local_snapshot', staleLocal: true });
  const staleSnapshot = await mockSnapshot(page);
  expect(staleSnapshot.calls.some(call => call.command === 'record_sync_remote_unchanged')).toBe(true);
  expect(staleSnapshot.calls.some(call => call.command === 'commit_sync_result')).toBe(false);
  expect(staleSnapshot.records[0].notes).toBe('edited during PROPFIND');

  expect((await runSync(page)).ok).toBe(true);
  const recovered = await mockSnapshot(page);
  expect(recovered.webdavV3Remote?.records[0].notes).toBe('edited during PROPFIND');
  expect(recovered.calls.some(call => call.command === 'webdav_request' && call.args.method === 'PUT')).toBe(true);
});

test('@conditional-pull JianGuoYun unquoted GET ETag uses quoted DAV preflight', async ({ page }) => {
  const local = record('jianguoyun-shape');
  const baseline = payload([local]);
  await setupMockIpc(page, {
    records: [local], webdavV3Remote: baseline, webdavV3Etag: '"dav-quoted"',
    webdavGetEtag: 'dav-unquoted', webdavPropfindEtag: '"dav-quoted"',
    settings: cleanConditionalSettings(baseline, '"dav-quoted"'),
  });
  await page.goto('/');

  expect((await runSync(page)).ok).toBe(true);
  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.some(call => call.command === 'webdav_request'
    && call.args.method === 'PROPFIND' && String(call.args.url).endsWith('records-v3.json'))).toBe(true);
  expect(snapshot.calls.some(call => call.command === 'webdav_request'
    && call.args.method === 'GET' && String(call.args.url).endsWith('records-v3.json'))).toBe(false);
  expect(snapshot.calls.filter(call => call.command === 'webdav_request' && call.args.method === 'PUT')).toHaveLength(0);
});

test('@conditional-pull 304 without response ETag preserves the stored validator', async ({ page }) => {
  const local = record('conditional-no-response-etag');
  const baseline = payload([local]);
  await setupMockIpc(page, {
    records: [local], webdavV3Remote: baseline, webdavV3Etag: 'W/"weak-etag"',
    webdavConditionalGet: true, webdavPropfindStatus: 405, omitConditionalGetEtag: true,
    settings: cleanConditionalSettings(baseline, 'W/"weak-etag"'),
  });
  await page.goto('/');

  expect((await runSync(page)).ok).toBe(true);
  const snapshot = await mockSnapshot(page);
  expect(snapshot.settings.sync_v3_remote_etag).toBe('W/"weak-etag"');
  expect(snapshot.calls.some(call => call.command === 'record_sync_remote_unchanged')).toBe(true);
});

test('@conditional-pull fallback 200 with same validator avoids merge and PUT', async ({ page }) => {
  const local = record('conditional-ignored');
  const baseline = payload([local]);
  await setupMockIpc(page, {
    records: [local], webdavV3Remote: baseline, webdavV3Etag: '"conditional-etag"', webdavPropfindStatus: 405,
    settings: cleanConditionalSettings(baseline),
  });
  await page.goto('/');

  expect((await runSync(page)).ok).toBe(true);
  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.some(call => call.command === 'commit_sync_result')).toBe(false);
  expect(snapshot.calls.some(call => call.command === 'record_sync_remote_unchanged')).toBe(true);
});

test('@conditional-pull semantic no-op ignores remote entity array order', async ({ page }) => {
  const first = record('semantic-first');
  const second = record('semantic-second');
  const baseline = payload([second, first]);
  const remote = { ...payload([first, second]), revision: 2, commitId: 'semantic-order-change' };
  await setupMockIpc(page, {
    records: [second, first], webdavV3Remote: remote, webdavV3Etag: '"semantic-new"', webdavPropfindStatus: 405,
    settings: cleanConditionalSettings(baseline, '"semantic-old"'),
  });
  await page.goto('/');

  expect((await runSync(page)).ok).toBe(true);
  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.some(call => call.command === 'commit_sync_result')).toBe(true);
  expect(snapshot.calls.filter(call => call.command === 'webdav_request' && call.args.method === 'PUT')).toHaveLength(0);
});

test('@conditional-pull semantic no-op canonicalizes records, episodes, collections, and members', async ({ page }) => {
  const first = record('canonical-first');
  const second = record('canonical-second');
  const completionFirst = { id: 'completion-first', recordId: first.id, episodeNumber: 1, completedAt: '2026-01-01T00:00:00.000Z', createdAt: '2026-01-01T00:00:00.000Z', updatedAt: '2026-01-01T00:00:00.000Z', rev: 1, revActor: 'fixture' };
  const completionSecond = { ...completionFirst, id: 'completion-second', recordId: second.id, episodeNumber: 2, completedAt: null };
  const collectionFirst = { id: 'collection-first', name: 'First', normalizedName: 'first', description: null, sourceKind: 'manual' as const, sourceKey: null, collectionKind: 'manual' as const, orderMode: 'manual' as const, createdAt: '2026-01-01T00:00:00.000Z', updatedAt: '2026-01-01T00:00:00.000Z', rev: 1, revActor: 'fixture' };
  const collectionSecond = { ...collectionFirst, id: 'collection-second', name: 'Second', normalizedName: 'second' };
  const memberFirst = { id: 'member-first', collectionId: collectionFirst.id, recordId: first.id, position: 0, sourceKind: 'manual' as const, createdAt: '2026-01-01T00:00:00.000Z', updatedAt: '2026-01-01T00:00:00.000Z', rev: 1, revActor: 'fixture' };
  const memberSecond = { ...memberFirst, id: 'member-second', collectionId: collectionSecond.id, recordId: second.id, position: 1 };
  const recordTombstones = [{ id: 'tombstone-first', deletedAt: '2026-01-01T00:00:00.000Z', rev: 1, revActor: 'fixture' }, { id: 'tombstone-second', deletedAt: '2026-01-02T00:00:00.000Z', rev: 1, revActor: 'fixture' }];
  const baseline = payload([second, first], {
    schemaVersion: 6, episodeCompletions: [completionSecond, completionFirst],
    collections: [collectionSecond, collectionFirst], collectionMembers: [memberSecond, memberFirst],
    collectionTombstones: [], collectionMemberTombstones: [], tombstones: [...recordTombstones].reverse(),
  });
  const remote = { ...baseline, revision: 2, commitId: 'canonical-order-change' };
  await setupMockIpc(page, {
    records: [second, first], episodeCompletions: [completionSecond, completionFirst],
    collections: [collectionSecond, collectionFirst], collectionMembers: [memberSecond, memberFirst],
    webdavV3Remote: remote, webdavV3Etag: '"canonical-new"', webdavPropfindStatus: 405,
    settings: { ...cleanConditionalSettings(baseline, '"canonical-old"'), sync_v6_upgrade_confirmed: '1', sync_tombstones: JSON.stringify([...recordTombstones].reverse()) },
  });
  await page.goto('/');

  expect((await runSync(page)).ok).toBe(true);
  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.filter(call => call.command === 'webdav_request' && call.args.method === 'PUT')).toHaveLength(0);
  expect(snapshot.calls.some(call => call.command === 'commit_sync_result')).toBe(true);
});

test('@conditional-pull real collection member position change produces a conditional PUT', async ({ page }) => {
  const local = record('collection-sync-record');
  const collection = { id: 'collection-sync', name: '同步收藏集', normalizedName: '同步收藏集', description: null, sourceKind: 'manual' as const, sourceKey: null, collectionKind: 'manual' as const, orderMode: 'manual' as const, createdAt: '2026-01-01T00:00:00.000Z', updatedAt: '2026-01-01T00:00:00.000Z', rev: 1, revActor: 'fixture' };
  const baseMember = { id: 'collection-member-sync', collectionId: collection.id, recordId: local.id, position: 0, sourceKind: 'manual' as const, createdAt: '2026-01-01T00:00:00.000Z', updatedAt: '2026-01-01T00:00:00.000Z', rev: 1, revActor: 'fixture' };
  const baseline = payload([local], { schemaVersion: 6, episodeCompletions: [], collections: [collection], collectionMembers: [baseMember], collectionTombstones: [], collectionMemberTombstones: [] });
  const pendingMember = { ...baseMember, position: 2, rev: 2, revActor: 'fixture-local' };
  const pendingOutbox = {
    version: 1, pending: true, dirtyGeneration: 0, reasons: ['collection-member-update'], firstQueuedAt: '2026-01-01T00:00:00.000Z', lastQueuedAt: '2026-01-01T00:00:00.000Z',
  };
  await setupMockIpc(page, {
    records: [local], collections: [collection], collectionMembers: [pendingMember],
    webdavV3Remote: baseline, webdavV3Etag: '"collection-change"',
    settings: { ...cleanConditionalSettings(baseline), sync_v6_upgrade_confirmed: '1', sync_outbox_v1: JSON.stringify(pendingOutbox) },
  });
  await page.goto('/');

  expect((await runSync(page)).ok).toBe(true);
  const snapshot = await mockSnapshot(page);
  const put = snapshot.calls.find(call => call.command === 'webdav_request' && call.args.method === 'PUT');
  expect(put).toBeDefined();
  expect(JSON.parse(String(put?.args.body)).collectionMembers[0].position).toBe(2);
});

test('@conditional-pull changed ETag returns 200 and merges the new remote payload', async ({ page }) => {
  const local = record('conditional-changed');
  const remoteOnly = record('conditional-remote-new');
  const baseline = payload([local]);
  const changed = { ...payload([local, remoteOnly]), revision: 2, commitId: 'changed-commit' };
  await setupMockIpc(page, {
    records: [local], webdavV3Remote: changed, webdavV3Etag: '"new-etag"', webdavConditionalGet: true,
    settings: cleanConditionalSettings(baseline, '"old-etag"'),
  });
  await page.goto('/');

  expect((await runSync(page)).ok).toBe(true);
  const snapshot = await mockSnapshot(page);
  const v3Get = snapshot.calls.find(call => call.command === 'webdav_request'
    && call.args.method === 'GET' && String(call.args.url).endsWith('records-v3.json'));
  expect(v3Get?.args.ifNoneMatch).toBeNull();
  expect(snapshot.records.map(item => item.id)).toContain(remoteOnly.id);
  expect(snapshot.calls.some(call => call.command === 'commit_sync_result')).toBe(true);
});

const unconditionalCases: Array<{
  name: string;
  settings: (baseline: SyncPayloadV3) => Record<string, string>;
}> = [
  {
    name: 'publish intent pending',
    settings: baseline => ({
      ...cleanConditionalSettings(baseline),
      sync_publish_intent_v1: JSON.stringify({
        version: 1, commitId: 'pending', previousCommitId: baseline.commitId,
        expectedGeneration: 0, includedEntries: [], payloadFingerprint: 'pending-fingerprint',
        createdAt: '2026-08-30T00:00:00.000Z',
      }),
    }),
  },
  {
    name: 'staging non-empty',
    settings: baseline => ({
      ...cleanConditionalSettings(baseline),
      sync_staging_v1: JSON.stringify({
        version: 2, entries: [{ entityKind: 'record', id: 'staged', operation: 'upsert', base: null, local: {}, firstGeneration: 0, lastGeneration: 0 }],
      }),
    }),
  },
  {
    name: 'baseline missing',
    settings: () => ({ ...credentials, sync_v3_remote_etag: '"conditional-etag"' }),
  },
  {
    name: 'remote ETag missing',
    settings: baseline => {
      const settings = cleanConditionalSettings(baseline);
      delete (settings as { sync_v3_remote_etag?: string }).sync_v3_remote_etag;
      return settings;
    },
  },
  {
    name: 'stored remote ETag malformed',
    settings: baseline => cleanConditionalSettings(baseline, 'unsafe-unquoted-etag'),
  },
  {
    name: 'stored remote ETag has surrounding whitespace',
    settings: baseline => cleanConditionalSettings(baseline, ' "conditional-etag" '),
  },
];

for (const entry of unconditionalCases) {
  test(`@conditional-pull ${entry.name} uses unconditional GET`, async ({ page }) => {
    const local = record(`conditional-${entry.name}`);
    const baseline = payload([local]);
    await setupMockIpc(page, {
      records: [local], webdavV3Remote: baseline, webdavV3Etag: '"conditional-etag"',
      webdavConditionalGet: true, settings: entry.settings(baseline),
    });
    await page.goto('/');

    expect((await runSync(page)).ok).toBe(true);
    const snapshot = await mockSnapshot(page);
    const v3Get = snapshot.calls.find(call => call.command === 'webdav_request'
      && call.args.method === 'GET' && String(call.args.url).endsWith('records-v3.json'));
    expect(v3Get?.args.ifNoneMatch).toBeNull();
    expect(snapshot.calls.some(call => call.command === 'record_sync_remote_unchanged')).toBe(false);
  });
}

test('@conditional-pull pending outbox keeps the full GET and conditional PUT path', async ({ page }) => {
  const base = record('conditional-dirty');
  const local = record('conditional-dirty', { notes: 'local pending change', rev: 2, revActor: 'mock-device' });
  const baseline = payload([base]);
  await setupMockIpc(page, {
    records: [local], webdavV3Remote: baseline, webdavV3Etag: '"conditional-etag"', webdavConditionalGet: true,
    settings: {
      ...cleanConditionalSettings(baseline),
      sync_outbox_v1: JSON.stringify({
        version: 1, pending: true, dirtyGeneration: 0, reasons: ['record-update'],
        firstQueuedAt: '2026-08-30T00:00:00.000Z', lastQueuedAt: '2026-08-30T00:00:00.000Z',
      }),
    },
  });
  await page.goto('/');

  expect((await runSync(page)).ok).toBe(true);
  const snapshot = await mockSnapshot(page);
  const v3Get = snapshot.calls.find(call => call.command === 'webdav_request'
    && call.args.method === 'GET' && String(call.args.url).endsWith('records-v3.json'));
  expect(v3Get?.args.ifNoneMatch).toBeNull();
  expect(snapshot.calls.some(call => call.command === 'webdav_request' && call.args.method === 'PUT')).toBe(true);
  expect(snapshot.webdavV3Remote?.records[0].notes).toBe('local pending change');
});

test('@conditional-pull local mutation during 304 is rejected and retried through full upload', async ({ page }) => {
  const local = record('conditional-toctou');
  const baseline = payload([local]);
  await setupMockIpc(page, {
    records: [local], webdavV3Remote: baseline, webdavV3Etag: '"conditional-etag"',
    webdavConditionalGet: true, webdavPropfindStatus: 405, mutateLocalDuringConditionalGet: true,
    settings: cleanConditionalSettings(baseline),
  });
  await page.goto('/');
  await page.getByRole('button', { name: /云端同步：/ }).click();
  await page.getByRole('button', { name: '立即同步' }).click();

  await expect.poll(async () => (await mockSnapshot(page)).webdavV3Remote?.records[0].notes)
    .toBe('edited during conditional pull');
  const snapshot = await mockSnapshot(page);
  const v3Gets = snapshot.calls.filter(call => call.command === 'webdav_request'
    && call.args.method === 'GET' && String(call.args.url).endsWith('records-v3.json'));
  expect(v3Gets[0].args.ifNoneMatch).toBe('"conditional-etag"');
  expect(v3Gets.some(call => call.args.ifNoneMatch === null)).toBe(true);
  expect(snapshot.calls.some(call => call.command === 'webdav_request' && call.args.method === 'PUT')).toBe(true);
  expect(JSON.parse(snapshot.settings.sync_outbox_v1 as string).pending).toBe(false);
  expect(snapshot.records[0].notes).toBe('edited during conditional pull');
});

test('@conditional-pull 304 preserves unresolved conflicts', async ({ page }) => {
  const local = record('conditional-conflict');
  const baseline = payload([local]);
  const conflicts = [{
    id: local.id, kind: 'edit-edit', fields: ['notes'], base: local, local, remote: local,
    localDeleted: false, remoteDeleted: false, detectedAt: '2026-08-30T00:00:00.000Z',
  }];
  await setupMockIpc(page, {
    records: [local], webdavV3Remote: baseline, webdavV3Etag: '"conditional-etag"', webdavConditionalGet: true, webdavPropfindStatus: 405,
    settings: { ...cleanConditionalSettings(baseline), sync_v3_conflicts: JSON.stringify(conflicts) },
  });
  await page.goto('/');

  expect(await runSync(page)).toMatchObject({ ok: true, conflictCount: 1 });
  const snapshot = await mockSnapshot(page);
  expect(JSON.parse(snapshot.settings.sync_v3_conflicts as string)).toEqual(conflicts);
});

test('@conditional-pull changed legacy resource still blocks the 304 fast path', async ({ page }) => {
  const local = record('conditional-legacy-block');
  const baseline = payload([local]);
  await setupMockIpc(page, {
    records: [local], webdavV3Remote: baseline, webdavV3Etag: '"conditional-etag"', webdavConditionalGet: true,
    settings: { ...cleanConditionalSettings(baseline), sync_v2_source_fingerprint: '"legacy-older"' },
  });
  await page.goto('/');

  expect(await runSync(page)).toMatchObject({ ok: false, error: 'legacy_remote_changed' });
  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.some(call => call.command === 'record_sync_remote_unchanged')).toBe(false);
  expect(snapshot.calls.some(call => call.command === 'commit_sync_result')).toBe(false);
  expect(snapshot.calls.some(call => call.command === 'webdav_request' && call.args.method === 'PUT')).toBe(false);
});

test('@expected-sync-v3 durable outbox survives reload and resumes without losing the edit', async ({ page }) => {
  const original = record('崩溃恢复条目');
  await setupMockIpc(page, {
    records: [original],
    settings: {
      ...credentials,
      sync_interval: '300',
      sync_scheduler_v1: JSON.stringify({
        version: 1, paused: true, consecutiveFailures: 0, nextAttemptAt: null,
        lastAttemptAt: null, lastSuccessAt: null, lastErrorCode: null, lastRemoteCheckAt: null,
      }),
    },
  });
  await page.goto('/');
  await expect(page.getByText('崩溃恢复条目').first()).toBeVisible();

  await page.evaluate(async () => {
    await window.__TAURI_INTERNALS__.invoke('update_record', {
      id: '崩溃恢复条目', updates: { notes: '退出前已经提交到 SQLite' },
    });
  });
  expect(JSON.parse((await mockSnapshot(page)).settings.sync_outbox_v1 as string).pending).toBe(true);

  await page.reload();
  await expect(page.getByRole('button', { name: /云端同步：已暂停/ })).toBeVisible();
  expect(await webdavCallCount(page)).toBe(0);
  await page.getByRole('button', { name: /云端同步：已暂停/ }).click();
  await page.getByRole('button', { name: '恢复自动同步' }).click();

  await expect.poll(async () => JSON.parse((await mockSnapshot(page)).settings.sync_outbox_v1 as string).pending).toBe(false);
  const snapshot = await mockSnapshot(page);
  expect(snapshot.records[0].notes).toBe('退出前已经提交到 SQLite');
  expect(snapshot.calls.some(call => call.command === 'webdav_request' && call.args.method === 'PUT')).toBe(true);
});

test('@expected-sync-v3 focus actively pulls a remote-only record while the local outbox is clean', async ({ page }) => {
  const local = record('本地已有');
  const remoteOnly = record('其他设备新增');
  const baseline = payload([local]);
  await setupMockIpc(page, {
    records: [local],
    webdavV3Remote: payload([local, remoteOnly]),
    webdavV3Etag: '"focus-etag"',
    settings: {
      ...credentials,
      sync_v3_baseline: JSON.stringify(baseline),
      sync_outbox_v1: JSON.stringify({
        version: 1, pending: false, dirtyGeneration: 0, reasons: [], firstQueuedAt: null, lastQueuedAt: null,
      }),
    },
  });
  await page.goto('/');
  await expect(page.getByText('本地已有').first()).toBeVisible();
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await expect(page.getByText('其他设备新增').first()).toBeVisible();

  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.some(call => call.command === 'commit_sync_result')).toBe(true);
  expect(JSON.parse(snapshot.settings.sync_outbox_v1 as string).pending).toBe(false);
});

test('@expected-sync-v3 online recovery actively pulls after connectivity returns', async ({ page }) => {
  const local = record('联网前本地条目');
  const remoteOnly = record('联网恢复新增');
  const baseline = payload([local]);
  await setupMockIpc(page, {
    records: [local],
    webdavV3Remote: payload([local, remoteOnly]),
    webdavV3Etag: '"online-etag"',
    settings: {
      ...credentials,
      sync_v3_baseline: JSON.stringify(baseline),
      sync_outbox_v1: JSON.stringify({
        version: 1, pending: false, dirtyGeneration: 0, reasons: [], firstQueuedAt: null, lastQueuedAt: null,
      }),
    },
  });
  await page.goto('/');
  await expect(page.getByText('联网前本地条目').first()).toBeVisible();
  await page.evaluate(() => window.dispatchEvent(new Event('online')));
  await expect(page.getByText('联网恢复新增').first()).toBeVisible();

  const snapshot = await mockSnapshot(page);
  expect(snapshot.calls.some(call => call.command === 'commit_sync_result')).toBe(true);
  expect(JSON.parse(snapshot.settings.sync_outbox_v1 as string).pending).toBe(false);
});

test('@expected-sync-v3 transient startup failure persists backoff and avoids a request loop', async ({ page }) => {
  await setupMockIpc(page, {
    records: [record('等待重试')],
    webdavFailureStatus: 503,
    webdavFailureCount: 1,
    settings: {
      ...credentials,
      sync_outbox_v1: JSON.stringify({
        version: 1, pending: true, dirtyGeneration: 0, reasons: ['record-update'],
        firstQueuedAt: '2026-08-02T00:00:00.000Z', lastQueuedAt: '2026-08-02T00:00:00.000Z',
      }),
    },
  });
  await page.goto('/');
  await expect.poll(async () => JSON.parse((await mockSnapshot(page)).settings.sync_scheduler_v1 as string).lastErrorCode).toBe('http_503');
  const runtime = JSON.parse((await mockSnapshot(page)).settings.sync_scheduler_v1 as string);
  expect(runtime.nextAttemptAt).not.toBeNull();
  const count = await webdavCallCount(page);
  await page.waitForTimeout(700);
  expect(await webdavCallCount(page)).toBe(count);
});

test('@expected-sync-v3 authentication blocker keeps the outbox and does not auto-retry', async ({ page }) => {
  await setupMockIpc(page, {
    records: [record('认证阻断')],
    webdavFailureStatus: 401,
    webdavFailureCount: 1,
    settings: {
      ...credentials,
      sync_outbox_v1: JSON.stringify({
        version: 1, pending: true, dirtyGeneration: 0, reasons: ['record-update'],
        firstQueuedAt: '2026-08-02T00:00:00.000Z', lastQueuedAt: '2026-08-02T00:00:00.000Z',
      }),
    },
  });
  await page.goto('/');
  await expect.poll(async () => JSON.parse((await mockSnapshot(page)).settings.sync_scheduler_v1 as string).lastErrorCode).toBe('http_401');
  const snapshot = await mockSnapshot(page);
  expect(JSON.parse(snapshot.settings.sync_outbox_v1 as string).pending).toBe(true);
  expect(JSON.parse(snapshot.settings.sync_scheduler_v1 as string).nextAttemptAt).toBeNull();
  const count = await webdavCallCount(page);
  await page.waitForTimeout(700);
  expect(await webdavCallCount(page)).toBe(count);
});

test('@expected-sync-v3 startup retries a persisted conditional-write blocker through DAV getetag', async ({ page }) => {
  const remote = payload([record('升级后恢复同步')]);
  const pending = record('升级后恢复同步', {
    notes: '本地待上传修改',
    updatedAt: '2026-08-02T00:01:00.000Z',
    rev: 2,
    revActor: 'local-device',
  });
  await setupMockIpc(page, {
    records: [pending],
    webdavV3Remote: remote,
    webdavV3Etag: '"upgrade-etag"',
    omitGetEtag: true,
    settings: {
      ...credentials,
      sync_v3_baseline: JSON.stringify(remote),
      sync_outbox_v1: JSON.stringify({
        version: 1, pending: true, dirtyGeneration: 97, reasons: ['record-update'],
        firstQueuedAt: '2026-08-02T00:00:00.000Z', lastQueuedAt: '2026-08-02T00:00:00.000Z',
      }),
      sync_scheduler_v1: JSON.stringify({
        version: 1, paused: false, consecutiveFailures: 2, nextAttemptAt: null,
        lastAttemptAt: '2026-08-02T00:00:00.000Z', lastSuccessAt: null,
        lastErrorCode: 'conditional_write_unsupported', lastRemoteCheckAt: null,
      }),
    },
  });

  await page.goto('/');
  await expect.poll(async () => JSON.parse((await mockSnapshot(page)).settings.sync_outbox_v1 as string).pending).toBe(false);

  const snapshot = await mockSnapshot(page);
  const runtime = JSON.parse(snapshot.settings.sync_scheduler_v1 as string);
  expect(runtime.lastErrorCode).toBeNull();
  expect(snapshot.calls.some(call => call.command === 'webdav_request' && call.args.method === 'PROPFIND')).toBe(true);
  expect(snapshot.calls.some(call => call.command === 'webdav_request' && call.args.method === 'PUT')).toBe(true);
});
