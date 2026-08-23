import assert from "node:assert/strict";
import test from "node:test";

import {
  errorCategory,
  notifyOperationFailure,
  publicFailureMessage,
  reportOperationFailure,
} from "../feedback.ts";

test("publicFailureMessage returns a fixed user-safe message", () => {
  assert.equal(publicFailureMessage("保存记录"), "保存记录失败，请稍后重试。");
});

test("errorCategory exposes only the error type", () => {
  assert.equal(errorCategory(new TypeError("secret database detail")), "TypeError");
  assert.equal(errorCategory("raw backend error"), "string");
});

test("reportOperationFailure does not log the raw error message", () => {
  const originalError = console.error;
  const captured = [];
  console.error = (...args) => captured.push(args);

  try {
    reportOperationFailure("record.save", new Error("private filesystem path"));
  } finally {
    console.error = originalError;
  }

  assert.deepEqual(captured, [
    ["[record.save] operation failed", { errorCategory: "Error" }],
  ]);
});

test("key asynchronous failures produce generic user notifications", () => {
  const originalError = console.error;
  const capturedLogs = [];
  const capturedNotices = [];
  const operations = [
    ["Record.Add", "添加记录"],
    ["Record.Edit", "更新记录"],
    ["Record.Delete", "删除记录"],
    ["Records.Import", "导入记录"],
    ["Records.Restore", "恢复冲突记录"],
    ["Sync.Run", "WebDAV 同步"],
    ["Settings.Save", "保存设置"],
  ];
  console.error = (...args) => capturedLogs.push(args);

  try {
    for (const [scope, action] of operations) {
      const message = notifyOperationFailure(
        scope,
        action,
        new Error("SQL: credentials and private path"),
        (tone, notice) => capturedNotices.push({ tone, message: notice }),
      );
      assert.equal(message, `${action}失败，请稍后重试。`);
    }
  } finally {
    console.error = originalError;
  }

  assert.deepEqual(
    capturedNotices,
    operations.map(([, action]) => ({
      tone: "error",
      message: `${action}失败，请稍后重试。`,
    })),
  );
  assert.deepEqual(
    capturedLogs,
    operations.map(([scope]) => [
      `[${scope}] operation failed`,
      { errorCategory: "Error" },
    ]),
  );
});
