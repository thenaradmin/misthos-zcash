import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import { resolve } from 'node:path';
import { test, expect, URLS, E2E_DIR, REPO_ROOT, ROSTER_CSV, FIXTURE_UFVK, expectInViewport, expectNoHorizontalScroll, isMobile, logButtons, rosterRows } from './fixtures.ts';

const BIN = resolve(REPO_ROOT, 'target/debug/payroll');
const VARS = ['ZCASH_NETWORK', 'ZCASH_LWD_URL', 'ZCASH_WALLET_DB', 'ZCASH_WALLET_CLI', 'ZCASH_AUDITOR_UFVK'];

test.describe('desk (read-only employer dashboard at /#desk)', () => {
  test('Setup required: every unset var by name, the roster from the CSV, focus on the heading', async ({ page }, ti) => {
    await page.goto(URLS.setup + '/#desk');
    await expect(page.locator('#landing')).toBeHidden();
    await expect(page.locator('#desk-h')).toBeFocused();
    // A direct load of /#desk stays at the top: the nav is not scrolled away (bug 1).
    await page.waitForLoadState('load');
    await page.waitForTimeout(300);
    expect(await page.evaluate(() => scrollY)).toBeLessThan(5);
    await expect(page.locator('#desk-h')).toBeFocused();
    await expect(page.locator('.nav')).toBeInViewport();
    const env = page.locator('#env');
    await expect(env).toHaveClass(/env bad/);
    await expect(env.locator('h3')).toHaveText('Setup required');
    await expect(env.locator('.vars code')).toHaveText(VARS);
    await expect(page.locator('#net')).toHaveClass(/bad/);
    await expect(page.locator('#net')).toHaveText('Setup required');

    const rows = rosterRows();
    await expect(page.locator('#rpath')).toHaveText(`${rows.length} employees · roster.csv`);
    await expect(page.locator('#rpath')).toHaveAttribute('title', ROSTER_CSV);
    const tr = page.locator('#rosterbox tbody tr');
    await expect(tr).toHaveCount(rows.length);
    for (const [i, r] of rows.entries()) {
      const row = tr.nth(i);
      await expect(row.locator('td').nth(0)).toHaveText(`${String(i + 1).padStart(2, '0')}row${i + 2}`);
      await expect(row.locator('td').nth(1)).toContainText(r.name);
      await expect(row.locator('td').nth(1)).toContainText(r.memo);
      await expect(row.locator('td').nth(3)).toHaveText(`${r.zec} ZEC`);
      await expect(row.locator('td').nth(2)).toHaveAttribute('title', r.ua);
      if (isMobile(ti.project.name)) await expect(row.locator('td').nth(2)).toBeHidden();
      else await expect(row.locator('td').nth(2)).toHaveText(`${r.ua.slice(0, 14)}…${r.ua.slice(-6)}`);
    }
    await expect(page.locator('#rosterbox tfoot')).toContainText('0.04 ZEC');
    // The CLI commands name the real roster path, shell-quoted: it contains a space (bug 2).
    expect(ROSTER_CSV).toContain(' ');
    await expect(page.locator('.rp')).toHaveText([`'${ROSTER_CSV}'`, `'${ROSTER_CSV}'`]);
    await expect(page.locator('#desk-view pre.cmd').first()).toHaveText(`payroll pay '${ROSTER_CSV}' --two-step --label oct-2026`);
    await expectNoHorizontalScroll(page);
    await page.screenshot({ path: ti.outputPath(`desk-setup-required-${ti.project.name}.png`), fullPage: true });
  });

  test('Run payroll is disabled with its reason; the server answers POST /api/run with 410', async ({ page, request }) => {
    await page.goto(URLS.setup + '/#desk');
    const run = page.locator('#run');
    await expect(run).toBeDisabled();
    await expect(run).toHaveText('Run payroll · disabled here');
    await expect(run).toHaveAttribute('aria-describedby', 'run-why');
    await expect(page.locator('#run-why')).toContainText('pay --two-step');
    let posted = false;
    page.on('request', (r) => r.url().endsWith('/api/run') && (posted = true));
    await run.click({ force: true });
    await page.waitForTimeout(300);
    expect(posted, 'a disabled button sent a request').toBe(false);
    const r = await request.post(URLS.setup + '/api/run');
    expect(r.status()).toBe(410);
    expect((await r.json()).error).toContain('payroll pay <roster.csv> --two-step');
    const p = await request.get(URLS.setup + '/api/proof?employee=row2');
    expect(p.status()).toBe(410);
    expect(await p.text()).not.toContain('uviewtest');
  });

  test('Accountant\'s view without ZCASH_AUDITOR_UFVK shows Setup required; the button recovers', async ({ page, watch }, ti) => {
    watch.allow(/\/api\/audit$/, 400); // the server's honest answer: ZCASH_AUDITOR_UFVK not configured
    await page.goto(URLS.setup + '/#desk');
    const btn = page.getByRole('button', { name: "Open the accountant's view" });
    const resp = page.waitForResponse('**/api/audit');
    await btn.click();
    expect((await resp).status()).toBe(400);
    const alert = page.locator('#audit .alert');
    await expect(alert).toContainText('Setup required');
    await expect(alert).toContainText('ZCASH_AUDITOR_UFVK not configured');
    await expect(btn).toBeEnabled();
    await expect(btn).toHaveText("Open the accountant's view");
    await page.locator('.audit').screenshot({ path: ti.outputPath(`desk-audit-setup-${ti.project.name}.png`) });
  });

  test('configured desk: Chain config loaded; the accountant view shows the UFVK and the empty run list', async ({ page }, ti) => {
    await page.goto(URLS.configured + '/#desk');
    const env = page.locator('#env');
    await expect(env).toHaveClass(/env good/);
    await expect(env.locator('h3')).toHaveText('Chain config loaded');
    await expect(env).toContainText('testnet');
    await expect(env).toContainText('https://testnet.zec.rocks:443');
    await expect(page.locator('#net')).toHaveText('testnet · testnet.zec.rocks'); // URL.host drops the default :443
    await page.getByRole('button', { name: "Open the accountant's view" }).click();
    await expect(page.locator('#audit .ufvk')).toHaveText(FIXTURE_UFVK);
    // Empty state: the desk never pays, so this session has no runs.
    await expect(page.locator('#audit')).toContainText('No pay runs in this session (the desk does not pay)');
    await expect(page.locator('#audit table')).toHaveCount(0);
    await expectNoHorizontalScroll(page);
    await page.screenshot({ path: ti.outputPath(`desk-configured-${ti.project.name}.png`), fullPage: true });
  });

  test('mainnet is refused; a spending-key-looking auditor value is refused and never shown', async ({ page, watch }, ti) => {
    watch.allow(/\/api\/audit$/, 400);
    await page.goto(URLS.mainnet + '/#desk');
    const env = page.locator('#env');
    await expect(env.locator('h3')).toHaveText('Setup required');
    await expect(env).toContainText('ZCASH_NETWORK invalid: `mainnet`; only `testnet` is allowed');
    await expect(env.locator('.vars')).toHaveCount(0);
    await page.getByRole('button', { name: "Open the accountant's view" }).click();
    await expect(page.locator('#audit .alert')).toContainText('looks like a spending key; refusing to export it');
    expect(await page.content()).not.toContain('secret-extended-key');
    await page.locator('.audit').screenshot({ path: ti.outputPath(`desk-audit-refused-${ti.project.name}.png`) });
  });

  test('loading state: skeleton rows and "Checking setup…" until /api/state answers', async ({ page }, ti) => {
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    // Hold the real request (no fake answer): it goes to the real server once released.
    await page.route('**/api/state', async (route) => {
      await held;
      await route.continue();
    });
    await page.goto(URLS.setup + '/#desk');
    await expect(page.locator('#env h3')).toHaveText('Checking setup…');
    await expect(page.locator('#rosterbox .skel')).toHaveCount(3);
    await expect(page.locator('#net')).toHaveText('checking setup…');
    expect(await page.locator('#rosterbox .skel').first().evaluate((s) => getComputedStyle(s).animationName)).toBe('sk');
    await page.screenshot({ path: ti.outputPath(`desk-loading-${ti.project.name}.png`) });
    release();
    await expect(page.locator('#env h3')).toHaveText('Setup required');
    await expect(page.locator('#rosterbox .skel')).toHaveCount(0);
  });

  test('reduced motion: the loading skeleton does not shimmer', async ({ page }) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    await page.route('**/api/state', async (route) => {
      await held;
      await route.continue();
    });
    await page.goto(URLS.setup + '/#desk');
    expect(await page.locator('#rosterbox .skel').first().evaluate((s) => getComputedStyle(s).animationName)).toBe('none');
    release();
    await expect(page.locator('#env h3')).toHaveText('Setup required');
  });

  test('unreachable: the server stops before /api/state; the desk says so and the accountant button too', async ({ page, watch }, ti) => {
    // A server of our own on 8794; this test stops it (by its PID) while the page is loading.
    const port = 8794;
    const srv: ChildProcess = spawn(BIN, ['serve', '--addr', `127.0.0.1:${port}`], {
      env: { PATH: process.env.PATH!, PAYROLL_ROSTER_CSV: ROSTER_CSV },
      stdio: 'ignore',
    });
    try {
      await expect.poll(async () => (await fetch(`http://127.0.0.1:${port}/api/state`).catch(() => null))?.status ?? 0).toBe(200);
      watch.allowFailure(/\/api\/(state|audit)$/);
      await page.route('**/api/state', async (route) => {
        srv.kill('SIGTERM');
        await new Promise((r) => srv.once('exit', r));
        await route.continue(); // the real request now meets a closed port
      });
      await page.goto(`http://127.0.0.1:${port}/#desk`);
      const env = page.locator('#env');
      await expect(env.locator('h3')).toHaveText("Can't reach the desk API");
      await expect(env).toContainText('Is payroll serve still running?');
      await expect(page.locator('#net')).toHaveText('desk API unreachable');
      await expect(page.locator('#rosterbox')).toHaveText('No roster: the server did not answer.');
      await page.getByRole('button', { name: "Open the accountant's view" }).click();
      await expect(page.locator('#audit .alert')).toContainText("Can't reach the desk API");
      await expect(page.getByRole('button', { name: "Open the accountant's view" })).toBeEnabled();
      await expectNoHorizontalScroll(page);
      await page.screenshot({ path: ti.outputPath(`desk-unreachable-${ti.project.name}.png`), fullPage: true });
    } finally {
      if (srv.exitCode === null && srv.signalCode === null) srv.kill('SIGTERM');
    }
  });

  test('empty roster: the server refuses to start rather than show an empty desk', async () => {
    const r = spawnSync(BIN, ['serve', '--addr', '127.0.0.1:8795'], {
      env: { PATH: process.env.PATH!, PAYROLL_ROSTER_CSV: resolve(E2E_DIR, 'fixtures/empty-roster.csv') },
      encoding: 'utf8',
      timeout: 10_000,
    });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain('CSV has a header but no rows');
    const none = spawnSync(BIN, ['serve', '--addr', '127.0.0.1:8795'], { env: { PATH: process.env.PATH! }, encoding: 'utf8', timeout: 10_000 });
    expect(none.status).toBe(1);
    expect(none.stderr).toContain('PAYROLL_ROSTER_CSV not configured');
  });

  test('hash switches boards: back link, nav anchors, browser back/forward', async ({ page }, ti) => {
    await page.goto(URLS.setup + '/');
    await expect(page.locator('#desk-view')).toBeHidden();
    await page.locator('.nav').getByRole('link', { name: 'Open the desk' }).click();
    await expect(page.locator('#desk-view')).toBeVisible();
    await expect(page.locator('#desk-h')).toBeFocused();
    await expect.poll(() => page.evaluate(() => scrollY)).toBeLessThan(5);
    await page.getByRole('link', { name: '← The payslip' }).click();
    await expect(page.locator('#landing')).toBeVisible();
    await expect(page.locator('#desk-view')).toBeHidden();
    await page.goBack();
    await expect(page.locator('#desk-view')).toBeVisible();
    await page.goBack();
    await expect(page.locator('#landing')).toBeVisible();
    await page.goForward();
    await expect(page.locator('#desk-view')).toBeVisible();
    // From the desk, a section link goes back to the payslip at that section.
    if (!isMobile(ti.project.name)) {
      await page.locator('.nav ul').getByRole('link', { name: 'Proof', exact: true }).click();
      await expect(page.locator('#landing')).toBeVisible();
      await expectInViewport(page, '#proof-h');
    }
    // The chip links to the desk too (shown above 480 px).
    if (!isMobile(ti.project.name)) {
      await page.locator('#net').click();
      await expect(page.locator('#desk-view')).toBeVisible();
    }
  });

  test('no button labelled "log" exists on the desk; every desk button was exercised', async ({ page }) => {
    await page.goto(URLS.setup + '/#desk');
    await expect(page.locator('#env h3')).toHaveText('Setup required');
    expect(await logButtons(page)).toHaveLength(0);
    await expect(page.getByRole('button')).toHaveText(['Run payroll · disabled here', "Open the accountant's view"]);
  });
});
