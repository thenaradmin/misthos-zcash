import { test, expect, URLS, EMPLOYER_PUBKEY, bars, expectInViewport, expectNoHorizontalScroll, isMobile, logButtons } from './fixtures.ts';

const BASE = URLS.setup;
// The recorded 2026-10-20 run, as README "Live testnet run" lists it.
const TX = {
  alice: 'f0e6ce4569f9a310b3ab936672331f947c5cadb102c3d5b50bcf2181910a46dc',
  bob: '287318aa4e0f0c55ca8b6814cb56b51264ce9dbfd549fcc8ef3db57ea06fa65a',
  funding: 'e0a93ebccc788ba5d6570478b927ebf1758318c0012f2d95ddda6624ef2cca87',
};
const HEIGHT = { funding: 4521170, alice: 4521181, bob: 4521189 };
const PUBLIC = ['alice:shut', 'alice:shut', 'alice:shut', 'bob:shut', 'bob:shut', 'bob:shut', 'funding:shut', 'employer:shut'];
const ALICE = ['alice:open', 'alice:open', 'alice:open', 'bob:shut', 'bob:shut', 'bob:shut', 'funding:open', 'employer:shut'];
const BOB = ['alice:shut', 'alice:shut', 'alice:shut', 'bob:open', 'bob:open', 'bob:open', 'funding:open', 'employer:shut'];

test.describe('landing (the redacted payslip)', () => {
  test('loads the payslip; no horizontal scroll; the desk chip reads Setup required', async ({ page }, ti) => {
    await page.goto(BASE + '/');
    await expect(page).toHaveTitle('Misthos · shielded payroll');
    await expect(page.getByRole('heading', { level: 1 })).toContainText("Everyone's pay on chain");
    await expect(page.locator('#slip-title')).toHaveText('payroll-2026-10-demo');
    await expect(page.locator('#desk-view')).toBeHidden();
    // The chip asks the real server; this one has no chain vars.
    if (isMobile(ti.project.name)) await expect(page.locator('#net')).toBeHidden(); // hidden under 480 px by design
    else await expect(page.locator('#net')).toHaveText('Setup required');
    await expect(page.locator('#net')).toHaveClass(/bad/);
    await expectNoHorizontalScroll(page);
    await page.screenshot({ path: ti.outputPath(`landing-hero-${ti.project.name}.png`) });
  });

  test('no key shows bars; Alice\'s key lifts her line once; Bob\'s key; back to no key', async ({ page }, ti) => {
    await page.goto(BASE + '/');
    // Before the slip is in view long enough, nothing is lifted.
    await expect(page.locator('.seg button[data-view="public"]')).toHaveAttribute('aria-pressed', 'true');
    expect(await bars(page)).toEqual(PUBLIC);
    await expect(page.locator('#reading')).toContainText('No key.');
    await page.screenshot({ path: ti.outputPath(`slip-no-key-${ti.project.name}.png`) });

    // Bars are a fixed width per column, so their size leaks no name or amount length.
    const widths = await page.locator('#sliptable tbody tr').evaluateAll((trs) => trs.map((tr) => Array.from(tr.querySelectorAll('.x')).map((x) => Math.round(x.getBoundingClientRect().width))));
    expect(widths[0]).toEqual(widths[1]);

    // The slip scrolls into view → after ~0.9 s Alice's key lifts her line (the page's one animation).
    await page.locator('#sliptable').scrollIntoViewIfNeeded();
    await expect.poll(() => bars(page), { timeout: 5000 }).toEqual(ALICE);
    await expect(page.locator('.seg button[data-view="alice"]')).toHaveAttribute('aria-pressed', 'true');
    await expect(page.locator('tr[data-row="alice"]')).toHaveClass(/mine/);
    await expect(page.locator('tr[data-row="alice"] .lifted-note')).toBeVisible();
    await expect(page.locator('tr[data-row="bob"] .lifted-note')).toBeHidden();
    await expect(page.locator('[data-key="funding"] .v')).toHaveText('0.0201 ZEC, her note');
    await expect(page.locator('#reading')).toContainText("Alice's key.");
    // It animated (not the instant class) and ends with the bar scaled away.
    await expect(page.locator('.slip')).not.toHaveClass(/instant/);
    await expect
      .poll(() => page.locator('tr[data-row="alice"] .x').first().evaluate((x) => getComputedStyle(x, '::after').transform))
      .toMatch(/^matrix\(0, 0, 0, 1, /);
    // Screen readers: lifted values are exposed, still-redacted ones say "redacted".
    await expect(page.locator('tr[data-row="alice"] .x .v').first()).toHaveAttribute('aria-hidden', 'false');
    await expect(page.locator('tr[data-row="bob"] .x .v').first()).toHaveAttribute('aria-hidden', 'true');
    await expect(page.locator('tr[data-row="bob"] .x .sr').first()).toHaveText('redacted');
    await page.screenshot({ path: ti.outputPath(`slip-alice-${ti.project.name}.png`) });

    // Bob's key: only his line and the funding note; the run total stays hatched.
    await page.getByRole('button', { name: "Bob's viewing key" }).click();
    expect(await bars(page)).toEqual(BOB);
    await expect(page.locator('tr[data-row="bob"]')).toHaveClass(/mine/);
    await expect(page.locator('tr[data-row="alice"]')).not.toHaveClass(/mine/);
    await expect(page.locator('[data-key="funding"] .v')).toHaveText('0.0201 ZEC, his note');
    await expect(page.locator('#reading')).toContainText("Bob's key.");
    await page.screenshot({ path: ti.outputPath(`slip-bob-${ti.project.name}.png`) });

    // No key again; scrolling away and back does not lift it a second time.
    await page.getByRole('button', { name: 'No key · public chain' }).click();
    expect(await bars(page)).toEqual(PUBLIC);
    await expect(page.locator('#reading')).toContainText('No key.');
    await page.locator('#limits').scrollIntoViewIfNeeded();
    await page.locator('#sliptable').scrollIntoViewIfNeeded();
    await page.waitForTimeout(1500);
    expect(await bars(page)).toEqual(PUBLIC);

    // Alice's button by hand.
    await page.getByRole('button', { name: "Alice's viewing key" }).click();
    expect(await bars(page)).toEqual(ALICE);
    await expectNoHorizontalScroll(page);
  });

  test('funding hop: the run total is hatched and no employee key lifts it', async ({ page }) => {
    await page.goto(BASE + '/');
    const total = page.locator('[data-key="employer"]');
    await expect(total).toHaveClass(/emp/);
    expect(await total.evaluate((x) => getComputedStyle(x, '::after').backgroundImage)).toContain('repeating-linear-gradient');
    for (const who of ["Alice's viewing key", "Bob's viewing key", 'No key · public chain']) {
      await page.getByRole('button', { name: who }).click();
      await expect(total).not.toHaveClass(/open/);
    }
    await expect(page.locator('.slip-foot .small')).toContainText('The hatched bar is the run total');
    // The funding tx moved 2 x 2,010,000 zat: the cell is the pay total and says what was funded.
    await expect(total.locator('xpath=..')).toHaveAttribute('data-l', 'Run total (pay)');
    expect(await total.locator('.v').textContent()).toBe('0.04 ZECpay · 0.0402 ZEC funded');
  });

  test('reduced motion: Alice\'s line is lifted at once, with no transition', async ({ page }, ti) => {
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.goto(BASE + '/');
    expect(await bars(page)).toEqual(ALICE);
    await expect(page.locator('.slip')).toHaveClass(/instant/);
    const t = await page.locator('tr[data-row="alice"] .x').first().evaluate((x) => [getComputedStyle(x).transitionDuration, getComputedStyle(x, '::after').transitionDuration]);
    expect(t).toEqual(['0s', '0s']);
    await page.getByRole('button', { name: "Bob's viewing key" }).click();
    expect(await bars(page)).toEqual(BOB);
    await page.screenshot({ path: ti.outputPath(`slip-reduced-motion-${ti.project.name}.png`) });
  });

  test('payout txids and heights match the recorded run; explorer links name each full txid', async ({ page }) => {
    await page.goto(BASE + '/');
    for (const [row, txid] of Object.entries(TX)) {
      const a = page.locator(`tr[data-row="${row}"] .tx a`);
      await expect(a).toHaveAttribute('href', `https://testnet.zecblock.com/tx/${txid}`);
      await expect(a).toHaveAttribute('title', txid);
      await expect(a).toHaveText(`${txid.slice(0, 8)}…${txid.slice(-6)} ↗`);
      await expect(page.locator(`tr[data-row="${row}"] .blk`)).toHaveText(String(HEIGHT[row as keyof typeof HEIGHT]));
    }
  });

  test('who-can-read table: each key reads its own line; the employer reads all', async ({ page }) => {
    await page.goto(BASE + '/#keys');
    await expectInViewport(page, '#keys-h');
    const grid = await page.locator('.matrix tbody tr').evaluateAll((trs) =>
      trs.map((tr) => Array.from(tr.querySelectorAll('td')).map((td) => (td.querySelector('.reads') ? 'reads' : td.querySelector('.mini') ? 'bar' : '?'))),
    );
    expect(grid).toEqual([
      ['bar', 'bar', 'bar'],
      ['reads', 'bar', 'bar'],
      ['bar', 'reads', 'bar'],
      ['reads', 'reads', 'reads'],
    ]);
    await expect(page.locator('.matrix tbody th')).toHaveText([/Anyone on the chain/, /Alice's viewing key/, /Bob's viewing key/, /The employer/]);
  });

  test('block ruler: pins sit at +11 and +19 blocks, labels do not overlap', async ({ page }, ti) => {
    await page.goto(BASE + '/#blocks');
    await expectInViewport(page, '#blocks-h');
    await expect(page.locator('#blocks-h')).toHaveText(`Two steps, ${HEIGHT.alice - HEIGHT.funding} and ${HEIGHT.bob - HEIGHT.funding} blocks after funding.`);
    const g = await page.evaluate(() => {
      const track = document.querySelector('.track')!.getBoundingClientRect();
      const pins = Array.from(document.querySelectorAll('.pin')).map((p) => {
        const r = p.getBoundingClientRect();
        return (r.left + r.width / 2 - track.left) / track.width;
      });
      const ticks = Array.from(document.querySelectorAll('.ticks span')).map((s) => s.getBoundingClientRect());
      const overlap = ticks.some((a, i) => ticks.some((b, j) => i < j && a.right > b.left && b.right > a.left));
      const inside = ticks.every((t) => t.left >= track.left - 7 && t.right <= track.right + 7);
      return { pins, overlap, inside };
    });
    expect(g.pins[0]).toBeCloseTo(0, 2);
    expect(g.pins[1]).toBeCloseTo(11 / 19, 2);
    expect(g.pins[2]).toBeCloseTo(1, 2);
    expect(g.overlap, 'tick labels overlap').toBe(false);
    expect(g.inside, 'tick labels stay on the ruler').toBe(true);
    await expect(page.locator('.ticks strong')).toHaveText([String(HEIGHT.funding), String(HEIGHT.alice), String(HEIGHT.bob)]);
    await expect(page.locator('.ruler')).toHaveAttribute('aria-label', new RegExp(`4521170.*4521181 \\(11 blocks later\\).*4521189 \\(19 blocks later, ${HEIGHT.bob - HEIGHT.alice} after Alice\\)`));
    await page.locator('.ruler').screenshot({ path: ti.outputPath(`ruler-${ti.project.name}.png`) });
  });

  test('proof format: the signed fields are Alice\'s recorded values and the published key', async ({ page }) => {
    await page.goto(BASE + '/#proof');
    await expectInViewport(page, '#proof-h');
    const att = page.locator('.att');
    await expect(att).toContainText('zec-payroll-employer-attestation v1');
    await expect(att).toContainText('network=testnet');
    await expect(att).toContainText(`txid=${TX.alice}`);
    await expect(att).toContainText('amount_zat=2000000'); // 0.02 ZEC on the slip
    await expect(att).toContainText('pay_period=adhoc:payroll-2026-10-demo');
    await expect(att).toContainText(EMPLOYER_PUBKEY);
    await expect(page.locator('.rej li')).toHaveText(['the other employee\'s txid', 'the funding txid', 'a changed amount', 'a changed period', 'a changed signature', 'a different employer key']);
    await expectNoHorizontalScroll(page);
  });

  test('deductions list the limits, including the 410 on the desk', async ({ page }) => {
    await page.goto(BASE + '/#limits');
    await expectInViewport(page, '#limits-h');
    await expect(page.locator('.ded li')).toHaveCount(9);
    await expect(page.locator('.ded li').last()).toContainText('410 Gone');
    // Fingerprints next to the timing limit: headcount from the funding tx, and the fixed payout shape.
    await expect(page.locator('.ded li').nth(1)).toContainText('Timing.');
    await expect(page.locator('.ded li').nth(2)).toHaveText("Headcount shows. The funding transaction's public action count (3 for 2 employees) roughly reveals how many people were paid.");
    await expect(page.locator('.ded li').nth(3)).toHaveText('Every payout has the same shape. 1 note in, 1 output, 2 actions and a 10,000 zat fee, sent in roster order. That fixed shape helps link payouts to their funding transaction.');
    await expect(page.locator('.ded')).toContainText('Testnet only.');
  });

  test('nav links, hero buttons and the brand land on their targets', async ({ page }, ti) => {
    await page.goto(BASE + '/');
    if (isMobile(ti.project.name)) {
      await expect(page.locator('.nav ul')).toBeHidden(); // section links are hidden on phones by design
    } else {
      for (const [name, target] of [['Payslip', '#payslip'], ['Keys', '#keys-h'], ['Blocks', '#blocks-h'], ['Proof', '#proof-h'], ['Limits', '#limits-h']]) {
        await page.locator('.nav ul').getByRole('link', { name, exact: true }).click();
        await expectInViewport(page, target);
      }
    }
    await page.getByRole('link', { name: 'Misthos, home' }).click();
    await expect.poll(() => page.evaluate(() => scrollY)).toBeLessThan(5);
    await page.locator('.hero-cta').getByRole('link', { name: 'Read the recorded run' }).click();
    await expectInViewport(page, '#payslip');
    // Both "Open the desk" links switch to the desk.
    for (const where of ['.nav', '.hero-cta']) {
      await page.goto(BASE + '/');
      await page.locator(where).getByRole('link', { name: 'Open the desk' }).click();
      await expect(page).toHaveURL(/#desk$/);
      await expect(page.locator('#desk-view')).toBeVisible();
      await expect(page.locator('#landing')).toBeHidden();
    }
  });

  test('no button labelled "log" exists on the payslip', async ({ page }) => {
    await page.goto(BASE + '/');
    expect(await logButtons(page)).toHaveLength(0);
    // The buttons that do exist all work (covered above): the three key buttons.
    await expect(page.getByRole('button')).toHaveText(['No key · public chain', "Alice's viewing key", "Bob's viewing key"]);
  });
});
