// Browser checks for the friends page, invite links and the leaderboard scope picker.
import assert from 'node:assert/strict';
import {test} from 'node:test';
import type {Browser, BrowserContext, Page} from 'playwright';
import {chromium, fulfillAsset, page as built} from './support/web.ts';

interface Person {user: string; display: string}
interface Group {id: number; name: string; owner: boolean; invite: string | null; members: Person[]}
const people = {alice: {user: 'alice', display: 'Alice'}, bob: {user: 'bob', display: 'Bob'}, cleo: {user: 'cleo', display: 'Cleo <b>'}};

function state() {
  return {
    social: {groups: [{id: 3, name: 'Spanish club', owner: true, invite: 'abc123', members: [people.alice, people.bob]}] as Group[], friends: [] as Person[], incoming: [people.cleo], outgoing: [] as Person[], public: false},
    writes: [] as Record<string, unknown>[],
    boards: [] as string[],
  };
}

async function fixture(browser: Browser, width = 390) {
  const data = state();
  const context: BrowserContext = await browser.newContext({locale: 'en-US', viewport: {width, height: 850}});
  await context.route('**/*', async route => {
    const url = new URL(route.request().url()), pathname = url.pathname;
    if (['/friends', '/join/abc123', '/join/gone'].includes(pathname)) return route.fulfill({contentType: 'text/html', body: built('personal')});
    if (['/week', '/records'].includes(pathname)) return route.fulfill({contentType: 'text/html', body: built('index')});
    if (await fulfillAsset(route, pathname)) return;
    if (pathname === '/auth/status') return route.fulfill({json: {private_site: false, authenticated: true, registration: true, member: {user: 'alice'}}});
    if (pathname === '/api/social/alice') {
      if (route.request().method() === 'POST') {
        const body = route.request().postDataJSON() as Record<string, unknown>;
        data.writes.push(body);
        if (body.action === 'befriend' && body.user === 'cleo') { data.social.incoming = []; data.social.friends = [people.cleo]; }
        if (body.action === 'befriend' && body.user === 'nobody') return route.fulfill({status: 404, json: {error: 'There is nobody with that username.'}});
        if (body.action === 'visibility') data.social.public = body.public as boolean;
        if (body.action === 'remove') data.social.groups[0]!.members = [people.alice];
        if (body.action === 'join') data.social.groups.push({id: 4, name: 'Joined', owner: false, invite: null, members: [people.alice]});
      }
      return route.fulfill({json: data.social});
    }
    if (pathname === '/api/invites/abc123') return route.fulfill({json: {name: 'Spanish club', members: 2}});
    if (pathname === '/api/invites/gone') return route.fulfill({status: 404, json: {error: 'gone'}});
    if (pathname === '/api/leaderboard' || pathname === '/api/records') {
      data.boards.push(url.search);
      return route.fulfill({json: pathname === '/api/records' ? [] : [{user: 'alice', display: 'Alice', level: 1, xp_total: 10, week_xp: 10, xp: 10, period: 'week', periods: {}, streak: 1, streak_state: 'safe', day_ends_at: 0, today_reviews: 1}]});
    }
    if (pathname === '/api/week') return route.fulfill({json: {ends_at: Date.now() + 86400000, timezone: 'UTC'}});
    if (pathname === '/api/winners') return route.fulfill({json: {}});
    if (pathname === '/manifest.webmanifest') return route.fulfill({json: {name: 'AnkiQuest'}});
    if (pathname === '/icon.svg') return route.fulfill({contentType: 'image/svg+xml', body: '<svg xmlns="http://www.w3.org/2000/svg"/>'});
    return route.fulfill({status: 404, body: pathname});
  });
  const page: Page = await context.newPage(), errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  return {page, context, data, errors};
}

const noOverflow = async (page: Page) => {
  const size = await page.evaluate(() => ({width: innerWidth, scroll: document.documentElement.scrollWidth}));
  assert(size.scroll <= size.width, JSON.stringify(size));
};

test('friends, groups and invite links', async () => {
  const browser = await chromium.launch({headless: true, channel: process.env.PLAYWRIGHT_CHANNEL || 'msedge'});
  try {
    for (const width of [320, 1440]) {
      const f = await fixture(browser, width);
      await f.page.goto('http://ankiquest.test/friends');
      await f.page.getByRole('heading', {name: 'Groups', exact: true}).waitFor();
      assert.equal(await f.page.locator('b').count(), 0, 'display names are escaped');
      assert.equal(await f.page.locator('[data-invite-link]').inputValue(), 'http://ankiquest.test/join/abc123');
      await noOverflow(f.page);

      await f.page.getByRole('button', {name: 'Accept'}).click();
      await f.page.waitForFunction(() => document.getElementById('social-status')?.textContent === 'Saved.');
      assert.deepEqual(f.data.writes.at(-1), {action: 'befriend', user: 'cleo'});
      assert.match((await f.page.locator('.personal-settings').textContent())!, /Your friends.*Cleo/s);

      await f.page.locator('#friend-form [name="user"]').fill('Nobody');
      await f.page.locator('#friend-form button[type="submit"]').click();
      await f.page.waitForFunction(() => document.getElementById('social-status')?.textContent === 'There is nobody with that username.');
      assert.deepEqual(f.data.writes.at(-1), {action: 'befriend', user: 'nobody'});

      await f.page.locator('[data-social-public]').check();
      await f.page.waitForFunction(() => document.getElementById('social-status')?.textContent === 'Saved.');
      assert.deepEqual(f.data.writes.at(-1), {action: 'visibility', public: true});

      const remove = f.page.locator('.social-group button', {hasText: 'Remove'});
      await remove.click();
      assert.equal(await f.page.locator('.social-group [data-armed]').textContent(), 'Tap again to confirm');
      assert.equal(f.data.writes.at(-1)!.action, 'visibility', 'the first tap only asks for confirmation');
      await f.page.locator('.social-group [data-armed]').click();
      await f.page.waitForFunction(() => document.querySelectorAll('.social-group .list-row').length === 1);
      assert.deepEqual(f.data.writes.at(-1), {action: 'remove', group: 3, member: 'bob'});

      await f.page.goto('http://ankiquest.test/join/abc123');
      await f.page.getByRole('heading', {name: 'Spanish club'}).waitFor();
      await f.page.locator('#join-group').click();
      await f.page.waitForURL('**/friends');
      assert.deepEqual(f.data.writes.at(-1), {action: 'join', invite: 'abc123'});

      await f.page.goto('http://ankiquest.test/join/gone');
      await f.page.getByRole('heading', {name: 'This invite link is not valid any more.'}).waitFor();
      assert.deepEqual(f.errors, []);
      await f.context.close();
    }
  } finally {
    await browser.close();
  }
});

test('the leaderboard and records follow the chosen scope', async () => {
  const browser = await chromium.launch({headless: true, channel: process.env.PLAYWRIGHT_CHANNEL || 'msedge'});
  try {
    const f = await fixture(browser);
    await f.page.goto('http://ankiquest.test/week');
    await f.page.locator('#board-scope').waitFor();
    assert.deepEqual(await f.page.locator('#board-scope option').allTextContents(), ['Friends and groups', 'Everyone', 'Spanish club']);
    assert.equal(f.data.boards.at(-1), '?period=week', 'the server picks the default scope');
    await f.page.locator('#board-scope').selectOption('group:3');
    await f.page.waitForFunction(() => (document.getElementById('board-scope') as HTMLSelectElement | null)?.value === 'group:3');
    assert.equal(f.data.boards.at(-1), '?period=week&scope=group%3A3');
    await f.page.goto('http://ankiquest.test/records');
    await f.page.locator('#board-scope').waitFor();
    assert.equal(f.data.boards.at(-1), '?scope=group%3A3', 'the choice carries over to records');
    await noOverflow(f.page);
    assert.deepEqual(f.errors, []);
    await f.context.close();
  } finally {
    await browser.close();
  }
});
