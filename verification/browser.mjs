import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { writeFile, mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { createPreviewServer } from '../app/scripts/dev.mjs';
const output = fileURLToPath(new URL('./local/', import.meta.url));
await mkdir(output, { recursive: true });
const server = createPreviewServer();
await new Promise((done) => server.listen(0, '127.0.0.1', done));
const browser = await chromium.launch({ ...(process.env.TANDEM_TEST_BROWSER ? { executablePath: process.env.TANDEM_TEST_BROWSER } : {}), headless: true, args: ['--no-sandbox', '--disable-dev-shm-usage'] });
const checks = [];
const record = (name) => checks.push({ name, status: 'passed' });
const base = `http://127.0.0.1:${server.address().port}/`;
try {
  const preview = await browser.newPage({ viewport: { width: 1100, height: 900 } });
  const errors = []; preview.on('pageerror', (error) => errors.push(error.message));
  await preview.goto(base); await preview.waitForFunction(() => document.querySelector('#environment').textContent.includes('Просмотр интерфейса'));
  assert.equal(await preview.locator('[data-command]:enabled').count(), 0); record('browser preview disables every backend action');
  assert.equal(errors.length, 0); record('real frontend loads without JavaScript exceptions');
  await preview.locator('details summary').click(); assert.equal(await preview.locator('.advanced-content').isVisible(), true); record('advanced operations expand correctly');
  assert.equal(await preview.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true); record('desktop has no horizontal overflow');
  await preview.setViewportSize({ width: 390, height: 844 });
  assert.equal(await preview.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true); record('390px expanded layout has no horizontal overflow');
  await preview.close();

  const context = await browser.newContext({ viewport: { width: 1100, height: 900 }, colorScheme: 'light' });
  await context.addInitScript(() => {
    // Test-only boundary fixture. The application contains no simulated backend.
    window.__calls = []; window.__failNext = false; window.__delay = 0;
    const config = { schema_version: 1, game_filter: 'disabled', ipset_filter: 'loaded', check_updates_on_start: false, request_timeout_secs: 8, targets: ['https://discord.com/'], hosts_allowed_suffixes: ['discord.com'] };
    const snapshot = { schema_version: 1, app_version: '0.2.0', root: 'C:\\Program Files\\TandemWorkbench', initialized: true, administrator: true, config, bundle: { tag: 'TEST-FIXTURE-1.0', sha256: 'a'.repeat(64) }, strategies: ['general.bat', '<img src=x onerror=alert(1)>.bat'], service: { state: 'stopped' }, service_owned: true, legacy_service_present: false, driver_present: true, pending_recovery: [], hosts_restore_available: false, bundle_rollback_available: true };
    window.__TAURI__ = { core: { invoke: async (command, { action }) => {
      window.__calls.push({ command, action });
      if (window.__delay) await new Promise((done) => setTimeout(done, window.__delay));
      if (window.__failNext) { window.__failNext = false; throw new Error('Access denied — TEST FIXTURE'); }
      if (action.type === 'get_dashboard') return structuredClone(snapshot);
      if (action.type === 'save_config') { snapshot.config = structuredClone(action.config); return { ok: true }; }
      if (action.type === 'install_service' || action.type === 'start_service') snapshot.service.state = 'running';
      if (action.type === 'stop_service') snapshot.service.state = 'stopped';
      if (action.type === 'remove_service') snapshot.service = null;
      if (action.type === 'export_support') return { report_schema: 1, redacted: true, os: 'TEST-FIXTURE' };
      return { ok: true, test_fixture: true };
    } }, event: { listen: async () => () => {} } };
  });
  const page = await context.newPage(); const ipcErrors = []; page.on('pageerror', (error) => ipcErrors.push(error.message));
  await page.goto(base); await page.waitForFunction(() => !document.querySelector('#install').disabled);
  assert.equal(await page.locator('#strategy img').count(), 0); record('strategy names are rendered as text, not HTML');
  await page.locator('#install').click(); await page.locator('#confirm-dialog').waitFor({ state: 'visible' });
  assert.equal(await page.evaluate(() => window.__calls.some((entry) => entry.action.type === 'install_service')), false); record('destructive action requires explicit confirmation');
  await page.locator('#confirm-dialog button[value=cancel]').click();
  assert.equal(await page.evaluate(() => window.__calls.some((entry) => entry.action.type === 'install_service')), false); record('cancelled confirmation does not invoke backend');
  await page.locator('#install').click(); await page.locator('#confirm-dialog button[value=confirm]').click();
  await page.waitForFunction(() => document.querySelector('#service-state').textContent === 'Работает'); record('confirmed service operation refreshes the true returned state');
  await page.evaluate(() => { window.__failNext = true; }); await page.locator('#diagnostics').click();
  await page.waitForFunction(() => document.querySelector('#feedback').textContent.includes('Access denied'));
  assert.equal(await page.locator('#diagnostics').isEnabled(), true); record('backend errors remain visible and release the busy gate');
  assert.match(await page.locator('#result').textContent(), /Access denied/); record('failed operation replaces stale success JSON');
  await page.screenshot({ path: `${output}/desktop-error.png`, fullPage: true });
  await page.evaluate(() => { window.__delay = 300; }); await page.locator('#diagnostics').click();
  assert.equal(await page.locator('#install').isDisabled(), true); record('requests disable competing controls');
  await page.waitForFunction(() => document.body.getAttribute('aria-busy') === 'false');
  await page.evaluate(() => { window.__delay = 0; });
  await page.locator('#support').click(); await page.waitForFunction(() => document.querySelector('#result').textContent.includes('redacted'));
  const downloaded = page.waitForEvent('download'); await page.locator('#download-report').click(); const file = await downloaded;
  assert.equal(file.suggestedFilename(), 'tandem-result.json'); record('JSON result download produces a real file');
  await page.emulateMedia({ colorScheme: 'dark' }); await page.screenshot({ path: `${output}/desktop-dark.png`, fullPage: true });
  await page.setViewportSize({ width: 390, height: 844 }); await page.locator('#remove').click();
  await page.locator('#confirm-dialog').waitFor({ state: 'visible' });
  assert.equal(await page.evaluate(() => { const r = document.querySelector('#confirm-dialog').getBoundingClientRect(); return r.left >= 0 && r.right <= innerWidth && r.top >= 0 && r.bottom <= innerHeight; }), true);
  await page.screenshot({ path: `${output}/mobile-dialog.png` }); record('confirmation dialog fits 390px viewport in dark mode');
  await page.locator('#confirm-dialog button[value=cancel]').click();
  assert.equal(ipcErrors.length, 0); record('interactive fixture generates no JavaScript exceptions');
  await context.close();
  await writeFile(`${output}/browser-results.json`, JSON.stringify({ harness: 'Playwright + local Chromium; simulated IPC only, not Windows service execution', checks }, null, 2));
  console.log(JSON.stringify({ passed: checks.length, checks }, null, 2));
} finally { await browser.close(); server.closeAllConnections(); await new Promise((done) => server.close(done)); }
