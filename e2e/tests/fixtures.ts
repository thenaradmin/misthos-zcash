import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test as base, expect, type Page } from '@playwright/test';

export { URLS } from '../global-setup.ts';
export const E2E_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));
export const REPO_ROOT = resolve(E2E_DIR, '..');
export const ROSTER_CSV = resolve(E2E_DIR, 'fixtures/roster.csv');
export const FIXTURE_UFVK = readFileSync(resolve(E2E_DIR, 'fixtures/auditor-ufvk.txt'), 'utf8').trim();
/** The published employer key: the first non-comment line of docs/employer-pubkey.txt. */
export const EMPLOYER_PUBKEY = readFileSync(resolve(REPO_ROOT, 'docs/employer-pubkey.txt'), 'utf8')
  .split('\n')
  .find((l) => l.trim() && !l.startsWith('#'))!
  .trim();

type Allowed = { url: RegExp; status: number };

/**
 * Every test gets a `watch` that records console errors, page errors, failed same-origin requests and
 * same-origin HTTP errors, and asserts at the end that there were none. A test may `allow()` a status
 * on a URL when that status is the product's honest answer (400 "ZCASH_AUDITOR_UFVK not configured"),
 * or `allowFailure()` a URL in the one test that stops the server on purpose. Nothing else is excused.
 */
export const test = base.extend<{ watch: { allow: (url: RegExp, status: number) => void; allowFailure: (url: RegExp) => void } }>({
  watch: [
    async ({ page }, use, testInfo) => {
      const allowed: Allowed[] = [];
      const mayFail: RegExp[] = [];
      const problems: string[] = [];
      const isAllowed = (url: string, status: number) => allowed.some((a) => a.status === status && a.url.test(url));
      const sameOrigin = (url: string) => {
        try {
          return new URL(url).origin === new URL(page.url()).origin;
        } catch {
          return false;
        }
      };
      page.on('pageerror', (e) => problems.push(`pageerror: ${e.message}`));
      page.on('console', (m) => {
        if (m.type() !== 'error') return;
        const loc = m.location().url || '';
        const st = /status of (\d{3})/.exec(m.text());
        if (st && isAllowed(loc, Number(st[1]))) return;
        if (/net::ERR_/.test(m.text()) && mayFail.some((r) => r.test(loc))) return;
        problems.push(`console.error: ${m.text()} @ ${loc}`);
      });
      page.on('requestfailed', (r) => {
        if (!sameOrigin(r.url())) return;
        const why = r.failure()?.errorText || '';
        if (why === 'net::ERR_ABORTED') return;
        if (mayFail.some((re) => re.test(r.url()))) return;
        problems.push(`requestfailed: ${r.method()} ${r.url()} ${why}`);
      });
      page.on('response', (r) => {
        if (!sameOrigin(r.url()) || r.status() < 400) return;
        if (isAllowed(r.url(), r.status())) return;
        problems.push(`HTTP ${r.status()}: ${r.request().method()} ${r.url()}`);
      });
      await use({ allow: (url, status) => allowed.push({ url, status }), allowFailure: (url) => mayFail.push(url) });
      if (problems.length) await testInfo.attach('problems', { body: problems.join('\n') });
      expect(problems, 'console errors / failed same-origin requests').toEqual([]);
    },
    { auto: true },
  ],
});

export { expect };

/** No horizontal page scroll (the 375 px check), and no element sticking out past the right edge. */
export async function expectNoHorizontalScroll(page: Page) {
  const r = await page.evaluate(() => {
    const cw = document.documentElement.clientWidth;
    const out: string[] = [];
    for (const el of Array.from(document.querySelectorAll('body *'))) {
      const h = el as HTMLElement;
      if (h.closest('[hidden]') || h.classList.contains('sr') || h.closest('.sr')) continue;
      const b = h.getBoundingClientRect();
      if (b.width && b.right > cw + 1) out.push(`${h.tagName.toLowerCase()}.${h.className || ''} right=${Math.round(b.right)}`);
    }
    return { sw: document.documentElement.scrollWidth, bsw: document.body.scrollWidth, cw, out: out.slice(0, 8) };
  });
  expect(r.sw, `page scrollWidth ${r.sw} > clientWidth ${r.cw}`).toBeLessThanOrEqual(r.cw);
  expect(r.bsw, `body scrollWidth ${r.bsw} > clientWidth ${r.cw}`).toBeLessThanOrEqual(r.cw);
  expect(r.out, 'elements past the right edge').toEqual([]);
}

/** The element's top is near the top of the viewport (an anchor jump landed). */
export async function expectInViewport(page: Page, selector: string) {
  await expect
    .poll(
      async () =>
        page.evaluate((sel) => {
          const r = document.querySelector(sel)!.getBoundingClientRect();
          return r.top >= -2 && r.top < innerHeight * 0.6;
        }, selector),
      { message: `${selector} scrolled into view` },
    )
    .toBe(true);
}

/** Every element with role=button whose accessible name contains "log" (Nick: "Can't use the log button"). */
export async function logButtons(page: Page) {
  return page.getByRole('button', { name: /log/i }).all();
}

/** Which redaction bars are lifted, as `key:open|shut` in DOM order. */
export async function bars(page: Page) {
  return page.locator('#sliptable .x').evaluateAll((xs) => xs.map((x) => `${(x as HTMLElement).dataset.key}:${x.classList.contains('open') ? 'open' : 'shut'}`));
}

export const isMobile = (name: string) => name === 'mobile';

/** Parse the fixture roster the way the desk shows it. */
export function rosterRows() {
  const [, ...rows] = readFileSync(ROSTER_CSV, 'utf8').trim().split('\n');
  return rows.map((l) => {
    const m = /^([^,]+),([^,]+),([^,]+),"(.*)"$/.exec(l)!;
    return { name: m[1], ua: m[2], zec: m[3], memo: m[4] };
  });
}
