import assert from "node:assert/strict";
import test from "node:test";

import { initializeApp, parseSyncInterval } from "../../../app/initialization.ts";

test("parseSyncInterval accepts the supported range", () => {
  assert.equal(parseSyncInterval("5"), 5);
  assert.equal(parseSyncInterval("300"), 300);
  assert.equal(parseSyncInterval("30"), 30);
});

test("parseSyncInterval falls back for invalid values", () => {
  assert.equal(parseSyncInterval(null), 30);
  assert.equal(parseSyncInterval("4"), 30);
  assert.equal(parseSyncInterval("301"), 30);
  assert.equal(parseSyncInterval("not-a-number"), 30);
});

test("initializeApp returns the loaded application state", async () => {
  const result = await initializeApp({
    readCredentials: async () => true,
    readSyncInterval: async () => "45",
    readPullInterval: async () => "30",
    readRecords: async () => [{ id: "record-1" }],
  });

  assert.deepEqual(result, {
    hasWebDAVCredentials: true,
    syncInterval: 45,
    pullIntervalMinutes: 30,
  });
});

test("initializeApp propagates initialization failures", async () => {
  await assert.rejects(
    initializeApp({
      readCredentials: async () => false,
      readSyncInterval: async () => null,
      readPullInterval: async () => null,
      readRecords: async () => {
        throw new Error("database unavailable");
      },
    }),
    /database unavailable/,
  );
});

test("initialization can be retried after a failed attempt", async () => {
  let attempts = 0;
  const dependencies = {
    readCredentials: async () => false,
    readSyncInterval: async () => null,
    readPullInterval: async () => null,
    readRecords: async () => {
      attempts += 1;
      if (attempts === 1) throw new Error("temporary database failure");
      return [];
    },
  };

  await assert.rejects(initializeApp(dependencies), /temporary database failure/);
  assert.deepEqual(await initializeApp(dependencies), {
    hasWebDAVCredentials: false,
    syncInterval: 30,
    pullIntervalMinutes: 15,
  });
  assert.equal(attempts, 2);
});
