# Verification evidence

This directory records what was executed, not what is merely configured in CI.

- `audit-findings.json`: 38 source audit items with locations in the original archive.
- `browser.mjs`: optional browser integration harness. Its injected Tauri object is explicitly a TEST FIXTURE and is never included in production frontend code.
- `browser-results.json`: generated results of actual browser checks, when present.
- `local-checks.json`: generated frontend/structural verification summary.
- `frontend-checks.log`: recorded Node syntax/test/build output.

## Reproduce browser checks

The application has no external npm dependencies. This **optional test harness**, not the app, uses Playwright. Install it only in a development/test environment, without saving a runtime dependency:

```sh
npm install --no-save --package-lock=false playwright
npx playwright install chromium
node verification/browser.mjs
```

`TANDEM_TEST_BROWSER` may identify an already installed Chromium-compatible executable. Otherwise Playwright's installed Chromium is used. Screenshots and JSON are written to `verification/local/` (gitignored); the harness starts/closes its own localhost server. It tests the real frontend plus a simulated IPC boundary, not real Windows APIs.

Do not confuse this with native acceptance. Run Cargo/Windows tests and the checklist in [VERIFICATION.md](../docs/VERIFICATION.md) before asserting native correctness. The initial revision environment had Node/Chromium/Python but no Cargo/rustc and could not reach package registries.
