import assert from 'node:assert/strict';
import test from 'node:test';
import { createWebDavTransport } from '../../../features/sync/infrastructure/webdavTransport.ts';
import { syncToWebDAV } from '../../../features/sync/services/syncService.ts';

const now = new Date('2026-08-22T00:00:00.000Z');

test('legacy webdav facade exports remain available without invoking Tauri on import', async () => {
  const facade = await import('../webdav.ts');
  for (const name of [
    'normalizeSyncTargetUrl', 'saveCreds', 'getCreds', 'clearCreds', 'hasCreds',
    'probeSyncTarget', 'syncToWebDAV', 'loadFromWebDAV', 'importLegacyChangesToConflictCenter',
    'getSyncConflicts', 'clearResolvedSyncConflicts', 'syncFailureMessage',
  ]) assert.equal(typeof facade[name], 'function', name);
  assert.equal(facade.normalizeSyncTargetUrl('https://example.test/dav/?secret=removed'), 'https://example.test/dav/');
});

test('WebDAV transport maps injected command calls without applying sync policy', async () => {
  const calls = [];
  const transport = createWebDavTransport(async (command, args) => {
    calls.push({ command, args });
    return { status: 200, body: null, etag: '"etag"', text: null };
  });

  await transport.request('GET', { username: 'u', password: 'p', url: 'https://example.test/dav/' }, 'proxy', 'records-v3.json');
  await transport.request('PUT', { username: 'u', targetId: 'target', targetEpoch: 4, url: 'https://example.test/dav/' }, null, 'records-v3.json', '{}', '"old"');

  assert.equal(calls[0].command, 'probe_webdav_request');
  assert.equal(calls[0].args.request.url, 'https://example.test/dav/records-v3.json');
  assert.equal(calls[0].args.request.proxy, 'proxy');
  assert.equal(calls[1].command, 'webdav_request');
  assert.equal(calls[1].args.request.targetEpoch, 4);
  assert.equal(calls[1].args.request.ifMatch, '"old"');
});

test('sync service accepts injected transport/database and preserves create CAS flow', async () => {
  const requests = [];
  const intents = [];
  const commits = [];
  const database = {
    getSettingAsync: async () => null,
    getSyncSnapshot: async () => ({
      targetId: null, targetEpoch: null, records: [], tombstones: [], episodeCompletions: [],
      collections: [], collectionMembers: [], collectionTombstones: [], collectionMemberTombstones: [],
      recordsGeneration: 7, baseline: null, deviceId: 'device-a', conflicts: [], remoteEtag: null,
      lastCommit: null, v2SourceFingerprint: null, outbox: {}, scheduler: {}, staging: {}, publishIntent: null,
    }),
    prepareSyncPublishIntent: async input => { intents.push(input); return input; },
    setSettingAsync: async () => true,
    commitSyncResult: async input => { commits.push(input); return { recordsGeneration: 8, recordCount: 0 }; },
  };
  const transport = {
    request: async (method, _creds, _proxy, resource, body, ifMatch, ifNoneMatch) => {
      requests.push({ method, resource, body, ifMatch, ifNoneMatch });
      if (method === 'MKCOL') return { status: 405, body: null, etag: null, text: null };
      if (method === 'GET' && resource === 'records-v3.json') return { status: 404, body: null, etag: null, text: null };
      if (method === 'GET' && resource === 'records.json') return { status: 404, body: null, etag: null, text: null };
      if (method === 'PUT') return { status: 201, body: null, etag: '"created"', text: null };
      throw new Error(`unexpected request ${method} ${resource}`);
    },
  };

  const result = await syncToWebDAV(
    { username: 'u', password: 'p', url: 'https://example.test/dav/' },
    undefined,
    { transport, database, now: () => now, uuid: () => 'commit-fixed', confirm: () => true },
  );

  assert.equal(result.ok, true);
  assert.deepEqual(requests.map(item => `${item.method}:${item.resource}`), [
    'MKCOL:records-v3.json', 'GET:records-v3.json', 'GET:records.json', 'PUT:records-v3.json',
  ]);
  assert.equal(intents[0].commitId, 'commit-fixed');
  assert.equal(commits[0].remoteEtag, '"created"');
  assert.equal(commits[0].lastCommit.commitId, 'commit-fixed');
});
