import { writeFileSync } from 'node:fs';
import { test, expect, URLS, FIXTURE_UFVK, ROSTER_CSV, expectNoHorizontalScroll, isMobile, logButtons, rosterRows } from './fixtures.ts';

// The payroll desk at /desk, against four real `payroll serve` binaries (stack.sh). Only 8794 runs with
// --payouts, and its wallet CLI is `false`, so every pay run stops at the CLI's own pre-broadcast check:
// nothing is ever sent from these tests.
const nav = (page, name: string) => page.getByRole('navigation', { name: 'Desk' }).getByRole('link', { name, exact: true });

test.describe('desk (/desk, the payroll desk)', () => {
  test('Setup required: every unset var by name; the roster from the CSV; Run payroll stays off', async ({ page }, ti) => {
    await page.goto(URLS.setup + '/desk');
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('Run payroll');
    await expect(page.locator('.eyebrow').first()).toHaveText('Private payroll processing');
    const setup = page.locator('#setup');
    await expect(setup).toContainText('Setup required');
    for (const v of ['ZCASH_NETWORK', 'ZCASH_LWD_URL', 'ZCASH_WALLET_DB', 'ZCASH_WALLET_CLI', 'ZCASH_AUDITOR_UFVK']) await expect(setup).toContainText(`${v} not configured`);
    await expect(page.locator('#status')).toHaveText('Setup required');
    const rows = rosterRows();
    await expect(page.locator('#roster tbody tr')).toHaveCount(rows.length);
    for (const r of rows) await expect(page.locator('#roster')).toContainText(r.name);
    await expect(page.locator('#k-n')).toHaveText(String(rows.length));
    await expect(page.locator('#k-pay')).toContainText('4,000,000');
    // Planning is offline and works here; running does not.
    await page.getByRole('button', { name: 'Review against caps' }).click();
    await page.getByRole('button', { name: 'Check the plan' }).click();
    await expect(page.locator('#plan')).toContainText('Within the caps');
    await expect(page.locator('#runbtn')).toBeDisabled();
    await expectNoHorizontalScroll(page);
    await page.screenshot({ path: ti.outputPath(`desk-setup-${ti.project.name}.png`), fullPage: true });
  });

  test('read-only desk: says so, and the server answers the payout endpoints with 410', async ({ page, request }) => {
    await page.goto(URLS.configured + '/desk');
    await expect(page.locator('#setup')).toContainText('Read-only desk');
    await expect(page.locator('#status')).toHaveText('Read-only');
    await page.getByRole('button', { name: 'Review against caps' }).click();
    await page.getByRole('button', { name: 'Check the plan' }).click();
    await expect(page.locator('#plan')).toContainText('Within the caps');
    await expect(page.locator('#runbtn')).toBeDisabled();
    const r = await request.post(URLS.configured + '/api/payroll/run', { headers: { 'X-Misthos-Desk': '1', 'Content-Type': 'application/json' }, data: { label: 'x' } });
    expect(r.status()).toBe(410);
    expect((await r.json()).error).toContain('--payouts');
    expect((await request.post(URLS.configured + '/api/run')).status()).toBe(410);
  });

  test('a foreign Host (DNS rebinding) gets 403 for the desk and its API; a cross-site POST is refused', async ({ request }) => {
    for (const path of ['/desk', '/api/state']) {
      const r = await request.get(URLS.payouts + path, { headers: { Host: 'attacker.example:8794' } });
      expect(r.status(), path).toBe(403);
    }
    const r = await request.post(URLS.payouts + '/api/payroll/run', { headers: { 'Content-Type': 'application/json', 'X-Misthos-Desk': '1', Origin: 'https://attacker.example' }, data: { label: 'x' } });
    expect(r.status()).toBe(403);
    const f = await request.post(URLS.payouts + '/api/payroll/run', { form: { label: 'x' } });
    expect(f.status()).toBe(403);
  });

  test('mainnet is refused and a spending-key-looking auditor value is never shown', async ({ page }) => {
    await page.goto(URLS.mainnet + '/desk');
    await expect(page.locator('#setup')).toContainText('Setup required');
    await expect(page.locator('#setup')).toContainText(/mainnet/i);
    await expect(page.locator('body')).not.toContainText('secret-extended-key');
  });

  test('caps: every violation is listed; fixing them unlocks Run payroll with the funding total', async ({ page, watch }) => {
    watch.allow(/\/api\/plan$/, 422);
    await page.goto(URLS.payouts + '/desk');
    await expect(page.locator('#status')).toHaveText('Payouts on');
    await page.getByRole('button', { name: 'Review against caps' }).click();
    await page.locator('#maxtotal').fill('0.03');
    await page.locator('#maxper').fill('0.01');
    await page.getByRole('button', { name: 'Check the plan' }).click();
    const plan = page.locator('#plan');
    await expect(plan).toContainText('over the caps');
    await expect(plan.locator('li')).toHaveCount(3); // two employees over the per-employee cap, and the run total
    await expect(page.locator('#runbtn')).toBeDisabled();
    await page.locator('#maxtotal').fill('0.05');
    await page.locator('#maxper').fill('0.02');
    await page.getByRole('button', { name: 'Check the plan' }).click();
    await expect(plan).toContainText('Within the caps');
    await expect(page.locator('#s-pay')).toHaveText('4,000,000 zat');
    await expect(page.locator('#s-fee')).toHaveText('20,000 zat');
    await expect(page.locator('#s-tot')).toHaveText('4,020,000 zat');
    await expect(page.locator('#runbtn')).toBeEnabled();
    await expect(page.locator('#runbtn')).toHaveText('Run payroll · 0.04 ZEC');
  });

  test('roster upload: errors are listed per line and nothing loads; a good CSV replaces the roster', async ({ page, watch }, ti) => {
    watch.allow(/\/api\/roster$/, 422);
    await page.goto(URLS.payouts + '/desk');
    const bad = ti.outputPath('bad.csv');
    writeFileSync(bad, 'name,ua,amountZEC\nEve,u1mainnetaddress,0.1\nMallory,utest1nope,0\n');
    await page.locator('#csvfile').setInputFiles(bad);
    await expect(page.locator('#rerr')).toContainText('nothing was loaded');
    await expect(page.locator('#rerr li')).toHaveCount(2);
    await expect(page.locator('#roster')).toContainText('Alice');
    await page.locator('#csvfile').setInputFiles(ROSTER_CSV);
    await expect(page.locator('.toast')).toContainText('Loaded roster.csv');
    await expect(page.locator('#rsrc')).toHaveText('roster.csv (uploaded in the desk)');
    await expect(page.locator('#rerr')).toBeHidden();
  });

  test('run: a wallet that is not ready stops the run before anything is sent, and the desk says so', async ({ page }, ti) => {
    const label = `e2e-${ti.project.name}-${Date.now()}`;
    await page.goto(URLS.payouts + '/desk');
    await page.getByRole('button', { name: 'Review against caps' }).click();
    await page.locator('#label').fill(label);
    await page.getByRole('button', { name: 'Check the plan' }).click();
    await page.locator('#runbtn').click();
    await expect(page.locator('#live-title')).toHaveText(`Run stopped · ${label}`, { timeout: 30_000 });
    await expect(page.locator('#live-sub')).toHaveText('Stopped before anything was sent: no funds moved.');
    await expect(page.locator('#outcome')).toContainText('step 1 NOT sent');
    await expect(page.locator('#feed li').first()).toContainText('Run stopped');
    await expect(page.locator('#periods')).toContainText(label);
    await expectNoHorizontalScroll(page);
    await page.screenshot({ path: ti.outputPath(`desk-stopped-${ti.project.name}.png`), fullPage: true });

    // Payslips: the period's register, nobody paid, so no payslip can be issued.
    await nav(page, 'Payslips').click();
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('Payslips');
    await expect(page.locator('#reg-sub')).toContainText(label);
    await expect(page.locator('#register tbody tr')).toHaveCount(2);
    await expect(page.locator('#register')).toContainText('not-started');
    for (const b of await page.locator('#keys [data-issue]').all()) await expect(b).toBeDisabled();

    // Accountant: the employer viewing key and the period's rows.
    await nav(page, 'Accountant').click();
    await expect(page.locator('#ac-key')).toContainText(FIXTURE_UFVK.slice(0, 28));
    await expect(page.locator('#actable')).toContainText('Alice');
    await expect(page.locator('#actable')).toContainText('4,000,000 zat');
    const dl = page.waitForEvent('download');
    await page.getByRole('button', { name: 'Download CSV' }).click();
    expect((await dl).suggestedFilename()).toBe(`misthos-${label}.csv`);
    await expectNoHorizontalScroll(page);
  });

  test('proof check: a file that is not a signed payslip is rejected by the real check', async ({ page }, ti) => {
    await page.goto(URLS.payouts + '/desk#verify');
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('Proof check');
    await expect(page.locator('#vbtn')).toBeDisabled();
    const f = ti.outputPath('not-a-proof.txt');
    writeFileSync(f, 'name=Alice\namount_zat=30000\n');
    await page.locator('#pffile').setInputFiles(f);
    await page.locator('#vtxid').fill('cd'.repeat(32));
    await page.locator('#vbtn').click();
    await expect(page.locator('#vres .result.bad')).toContainText('Rejected', { timeout: 30_000 });
    await expect(page.locator('#vres')).toContainText('not a zec-payroll proof file');
  });

  test('hash routes switch pages and focus each heading; back and forward work', async ({ page }, ti) => {
    await page.goto(URLS.payouts + '/desk');
    for (const [name, h] of [['Payslips', 'Payslips'], ['Proof check', 'Proof check'], ['Accountant', 'Accountant'], ['Run payroll', 'Run payroll']]) {
      await nav(page, name).click();
      await expect(page.getByRole('heading', { level: 1 })).toHaveText(h);
      await expect(page.locator('h1:visible')).toBeFocused();
      await expect(nav(page, name)).toHaveAttribute('aria-current', 'page');
      await expectNoHorizontalScroll(page);
    }
    await page.goBack();
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('Accountant');
    await page.goForward();
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('Run payroll');
    if (isMobile(ti.project.name)) await page.screenshot({ path: ti.outputPath('desk-mobile.png'), fullPage: true });
  });

  test('no button labelled "log" exists on the desk', async ({ page }) => {
    await page.goto(URLS.payouts + '/desk');
    expect(await logButtons(page)).toHaveLength(0);
  });
});
