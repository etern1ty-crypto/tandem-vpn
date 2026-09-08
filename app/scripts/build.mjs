import { mkdir, rm, copyFile, readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
export const appRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
export const publicFiles = Object.freeze(["index.html", "src/main.js", "src/ui-state.js", "src/styles.css"]);
export async function build(destination = join(appRoot, "dist")) {
  destination = resolve(destination);
  if (destination === appRoot || appRoot.startsWith(destination + "/") || destination === join(appRoot, "src")) throw new Error("Refusing unsafe build destination");
  await rm(destination, { recursive: true, force: true });
  for (const file of publicFiles) { await mkdir(dirname(join(destination, file)), { recursive: true }); await copyFile(join(appRoot, file), join(destination, file)); }
  const html = await readFile(join(destination, "index.html"), "utf8");
  if (/<(?:script|link|img|iframe)\b[^>]*(?:src|href)=["\'](?:https?:|\/\/)/i.test(html)) throw new Error("Frontend must not load remote runtime resources");
  return publicFiles;
}
if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  await build(); console.log("Built static frontend: 4 files, no npm dependencies, no CDN.");
}
