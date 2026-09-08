import { spawnSync } from "node:child_process";
const paths = ["src/main.js", "src/ui-state.js", "scripts/build.mjs", "scripts/dev.mjs", "scripts/check.mjs", "tests/ui.test.mjs", "tests/build.test.mjs"];
for (const path of paths) {
  const result = spawnSync(process.execPath, ["--check", path], { stdio: "inherit" });
  if (result.status !== 0) process.exit(result.status ?? 1);
}
console.log(`Syntax checked: ${paths.length} JavaScript modules.`);
