import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { appRoot, publicFiles } from "./build.mjs";
const types = { html: "text/html; charset=utf-8", js: "text/javascript; charset=utf-8", css: "text/css; charset=utf-8" };
export function createPreviewServer() {
  const server = createServer(async (request, response) => {
    const address = server.address();
    const port = typeof address === "object" && address ? address.port : 5173;
    const origin = `http://127.0.0.1:${port}`;
    const host = request.headers.host;
    if (host !== `127.0.0.1:${port}` && host !== `localhost:${port}`) { response.writeHead(403); response.end("Forbidden Host"); return; }
    if (request.headers.origin && request.headers.origin !== origin && request.headers.origin !== `http://localhost:${port}`) { response.writeHead(403); response.end("Forbidden Origin"); return; }
    if (request.method !== "GET" && request.method !== "HEAD") { response.writeHead(405, { Allow: "GET, HEAD" }); response.end(); return; }
    let path;
    try { path = new URL(request.url, origin).pathname; } catch { response.writeHead(400); response.end(); return; }
    if (path === "/") path = "/index.html";
    const file = publicFiles.find((file) => `/${file}` === path);
    if (!file) { response.writeHead(404); response.end("Not found"); return; }
    try {
      const content = await readFile(join(appRoot, file));
      response.writeHead(200, {
        "Content-Type": types[file.split(".").at(-1)], "Content-Length": content.length,
        "Cache-Control": "no-store", "X-Content-Type-Options": "nosniff", "Referrer-Policy": "no-referrer",
        "Content-Security-Policy": "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self' ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'",
      });
      response.end(request.method === "HEAD" ? undefined : content);
    } catch { response.writeHead(500); response.end("Local resource unavailable"); }
  });
  server.requestTimeout = 5000; server.headersTimeout = 5000; server.maxHeadersCount = 30;
  return server;
}
if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  const port = Number(process.env.TANDEM_PREVIEW_PORT ?? 5173);
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error("Invalid TANDEM_PREVIEW_PORT");
  const server = createPreviewServer();
  server.on("error", (error) => { console.error(error.message); process.exitCode = 1; });
  server.listen(port, "127.0.0.1", () => console.log(`Read-only preview: http://127.0.0.1:${port}`));
  let closing = false;
  function shutdown() {
    if (closing) return; closing = true;
    server.close(() => { process.exitCode = 0; }); server.closeIdleConnections();
    setTimeout(() => server.closeAllConnections(), 2000).unref();
  }
  process.on("SIGINT", shutdown); process.on("SIGTERM", shutdown);
}
