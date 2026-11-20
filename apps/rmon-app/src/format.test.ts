import { test } from "node:test";
import assert from "node:assert/strict";
import cases from "./bindings/format-cases.json" with { type: "json" };
import { cmdHint, fmtBytes } from "./format.ts";

test("fmtBytes matches the Rust reference (bindings/format-cases.json)", () => {
  assert.ok(cases.length > 0);
  for (const [bytes, expected] of cases as [number, string][]) {
    assert.equal(fmtBytes(bytes), expected, `fmtBytes(${bytes})`);
  }
});

test("cmdHint tells what a generic process runs", () => {
  assert.equal(cmdHint("node", "node /Users/x/.npm/_npx/9a2e/node_modules/mcp-memory-bank/dist/index.js"), "mcp-memory-bank");
  assert.equal(cmdHint("node", ["/opt/homebrew/bin/node", "/p/node_modules/.bin/jest", "--watch"]), "jest --watch");
  assert.equal(cmdHint("node", "node /p/node_modules/@playwright/mcp/cli.js"), "@playwright/mcp");
  assert.equal(cmdHint("python3", "python3 -m uvicorn app:app"), "uvicorn app:app");
  assert.equal(cmdHint("claude", "claude --resume"), "--resume");
  assert.equal(cmdHint("claude", "claude"), "");
});
