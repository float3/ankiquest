import assert from 'node:assert/strict';
import {test} from 'node:test';
import type {Browser} from 'playwright';
import {chromium, fulfillAsset, page as built} from './support/web.ts';

const DAY = 86_400_000, TODAY = Math.floor(Date.UTC(2026, 9, 30) / DAY);
const stats = {
  players: {total: 12, configured: 3, accounts: 8, synced: 1},
  active: {day: 4, week: 7, month: 9},
  days: Array.from({length: 30}, (_, index) => ({day: TODAY - 29 + index, reviews: index * 10, players: index % 4, signups: index === 29 ? 2 : 0})),
  groups: {count: 2, members: 5, sizes: [4, 2], friendships: 3},
  retention: {eligible: 6, retained: 3},
  generated_at: TODAY * DAY,
};

async function visit(browser: Browser, dev: boolean, width = 390) {
  const context = await browser.newContext({locale: 'en-US', viewport: {width, height: 1200}});
  const posted: {csrf: string | null; body: unknown}[] = [];
  const sent: unknown[] = [];
  await context.route('**/*', async route => {
    const request = route.request(), pathname = new URL(request.url()).pathname;
    if (pathname === '/dev') return route.fulfill({contentType: 'text/html', body: built('dev')});
    if (await fulfillAsset(route, pathname)) return;
    if (pathname === '/auth/status') return route.fulfill({json: {private_site: false, registration: true, authenticated: true, member: {user: 'hill', display: 'Hill'}, ...(dev ? {dev: true} : {})}});
    if (pathname === '/api/dev/hill/stats') return route.fulfill({json: stats});
    if (pathname === '/api/dev/hill/groups') return route.fulfill({json: [{id: 1, name: 'Book club', members: 4}, {id: 2, name: 'Night owls', members: 2}]});
    if (pathname === '/api/dev/hill/errors') return route.fulfill({json: [{at: TODAY * DAY, user: null, message: 'tick failed: disk full'}]});
    if (pathname === '/api/dev/hill/announcements' && request.method() === 'POST') {
      const body = request.postDataJSON();
      posted.push({csrf: request.headers()['x-ankiquest-csrf'] ?? null, body});
      sent.unshift({id: sent.length + 1, sender: 'hill', title: body.title, body: body.body, groups: (body.groups ?? []).map((id: number) => ({id, name: 'Book club'})), recipients: 4, created_at: TODAY * DAY});
      return route.fulfill({json: {id: sent.length, recipients: 4}});
    }
    if (pathname === '/api/dev/hill/announcements') return route.fulfill({json: sent});
    if (/^\/(aki|ankilope)\//.test(pathname)) return route.fulfill({status: 404, body: ''});
    return route.fulfill({status: 404, body: pathname});
  });
  const page = await context.newPage(), errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('dialog', dialog => dialog.accept());
  await page.goto('http://ankiquest.test/dev');
  await page.locator('main[aria-busy]').waitFor({state: 'detached'});
  const result = {
    link: await page.locator('.site-dev').isVisible(),
    text: (await page.locator('main').innerText()).replace(/\s+/g, ' '),
    rows: await page.locator('#dev-days tbody tr').count(),
    posted,
    status: '',
    listed: '',
    wide: await page.evaluate(() => document.documentElement.scrollWidth > innerWidth),
    errors,
  };
  if (dev) {
    await page.fill('input[name=title]', 'Update');
    await page.fill('textarea[name=body]', 'New quests\nthis week');
    await page.check('input[name=to][value=groups]');
    await page.check('input[name=group][value="1"]');
    await page.click('#dev-announce button[type=submit]');
    await page.locator('#dev-status', {hasText: 'Sent'}).waitFor();
    result.status = (await page.locator('#dev-status').textContent()) ?? '';
    result.listed = (await page.locator('#dev-announcements').innerText()).replace(/\s+/g, ' ');
    if (process.env.DEV_SCREENSHOTS) await page.screenshot({path: `${process.env.DEV_SCREENSHOTS}/dev-${width}.png`, fullPage: true});
  }
  await context.close();
  return result;
}

test('devs see totals and send announcements to chosen groups; others are told no', async () => {
  const browser = await chromium.launch({headless: true, channel: process.env.PLAYWRIGHT_CHANNEL || 'msedge'});
  try {
    const dev = await visit(browser, true);
    assert.equal(dev.link, true);
    assert.equal(dev.rows, 30);
    for (const text of ['12', '3 configured · 8 accounts · 1 sync only', '50%', 'largest 4 · median 3', 'tick failed: disk full', 'None yet.']) assert.ok(dev.text.includes(text), text);
    assert.deepEqual(dev.posted, [{csrf: '1', body: {title: 'Update', body: 'New quests this week', groups: [1]}}]);
    assert.equal(dev.status, 'Sent to 4 players.');
    assert.ok(dev.listed.includes('Update New quests this week'), dev.listed);
    assert.ok(dev.listed.includes('hill → Book club · 4 players'), dev.listed);
    assert.equal(dev.wide, false);
    assert.deepEqual(dev.errors, []);
    if (process.env.DEV_SCREENSHOTS) await visit(browser, true, 1200);
    const other = await visit(browser, false);
    assert.equal(other.link, false);
    assert.equal(other.text, 'This account is not a dev.');
    assert.deepEqual(other.posted, []);
    assert.deepEqual(other.errors, []);
  } finally {
    await browser.close();
  }
});
