import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { request as httpRequest } from "node:http";
import { build, publicFiles, appRoot } from "../scripts/build.mjs";
import { createPreviewServer } from "../scripts/dev.mjs";

test("offline build copies every public file without remote resources", async () => {
  const folder = await mkdtemp(join(tmpdir(), "tandem-build-"));
  try { await build(folder); for (const file of publicFiles) assert.deepEqual(await readFile(join(folder, file)), await readFile(join(appRoot, file))); }
  finally { await rm(folder, { recursive: true, force: true }); }
});
test("build refuses destructive destinations", async () => { await assert.rejects(build(appRoot)); });
function get(port, path, options = {}) {
  return new Promise((resolve, reject) => {
    const request = httpRequest({ hostname: "127.0.0.1", port, path, ...options }, (response) => {
      let body = ""; response.setEncoding("utf8"); response.on("data", (chunk) => { body += chunk; });
      response.on("end", () => resolve({ status: response.statusCode, headers: response.headers, body }));
    }); request.on("error", reject); request.end();
  });
}
test("preview serves only allowlisted files and protects localhost", async (t) => {
  const server = createPreviewServer(); await new Promise((done) => server.listen(0, "127.0.0.1", done));
  t.after(async () => { server.closeAllConnections(); await new Promise((done) => server.close(done)); });
  const port = server.address().port;
  const home = await get(port, "/"); assert.equal(home.status, 200); assert.match(home.headers["content-security-policy"], /object-src 'none'/);
  for (const path of ["/package.json", "/.env", "/../Cargo.toml", "/%2e%2e/Cargo.toml", "/src/../../Cargo.toml", "/src/main.js%00"]) assert.equal((await get(port, path)).status, 404, path);
  assert.equal((await get(port, "/", { headers: { Host: "attacker.example" } })).status, 403);
  assert.equal((await get(port, "/", { headers: { Origin: "https://attacker.example" } })).status, 403);
  assert.equal((await get(port, "/", { method: "POST" })).status, 405);
  assert.equal((await get(port, "/src/main.js", { method: "HEAD" })).body, "");
});
