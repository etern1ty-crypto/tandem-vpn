import test from "node:test";
import assert from "node:assert/strict";
import { STATE_LABELS, appendLog, createRequestGate, errorMessage, parseTargets, validateBundle } from "../src/ui-state.js";
test("trusted digest is mandatory and normalized", () => {
  assert.equal(validateBundle("1.9.9a", "A".repeat(64)).sha256, "a".repeat(64));
  for (const hash of ["", "a".repeat(63), "z".repeat(64)]) assert.throws(() => validateBundle("1.0", hash));
});
test("release tag cannot become a path", () => { for (const tag of ["", "../x", "v1?x=1", "v1\n", "latest"]) assert.throws(() => validateBundle(tag, "a".repeat(64))); });
test("targets enforce HTTPS and unique normalized URLs", () => {
  assert.deepEqual(parseTargets("https://example.com\n\nhttps://example.org/"), ["https://example.com", "https://example.org/"]);
  for (const text of ["", "http://example.com", "https://user:password@example.com", "https://example.com?token=1", "https://example.com:8443", "https://example.com\nhttps://example.com/"]) assert.throws(() => parseTargets(text));
});
test("target count has a hard upper bound", () => { assert.throws(() => parseTargets(Array.from({ length: 13 }, (_, n) => `https://x${n}.example.org`).join("\n"))); });
test("log is bounded and text remains inert", () => {
  let values = []; for (let n = 0; n < 220; n++) values = appendLog(values, `${n}`, "now");
  assert.equal(values.length, 200); assert.equal(values[0], "[now] 20");
  assert.equal(appendLog([], "<script>alert(1)</script>", "now")[0], "[now] <script>alert(1)</script>");
});
test("request gate rejects concurrent actions", async () => {
  let resolve; const changes = []; const gate = createRequestGate((state) => changes.push(state));
  const first = gate.run(() => new Promise((done) => { resolve = done; }));
  await assert.rejects(gate.run(async () => {})); resolve("done"); assert.equal(await first, "done");
  assert.deepEqual(changes, [true, false]); assert.equal(gate.busy, false);
});
test("failed requests release the gate", async () => {
  const gate = createRequestGate(); await assert.rejects(gate.run(async () => { throw new Error("denied"); }));
  assert.equal(gate.busy, false); assert.equal(await gate.run(async () => 42), 42);
});
test("structured failures are displayed honestly", () => { assert.equal(errorMessage(new Error("access denied")), "access denied"); assert.equal(errorMessage("failed"), "failed"); assert.equal(errorMessage({ code: 5 }), '{"code":5}'); });
test("all native service states have labels", () => { for (const state of ["stopped", "running", "paused", "unknown", "start_pending", "stop_pending", "pause_pending", "continue_pending"]) assert.equal(typeof STATE_LABELS[state], "string"); });
