// Browser checks for sharing: settings, the daily goal, wins on Today, and shared cards and cheers in activity.
import assert from 'node:assert/strict';
import {test, before, after} from 'node:test';
import type {TestContext} from 'node:test';
import type {Browser, Page} from 'playwright';
import {chromium, fulfillAsset, page as built} from './support/web.ts';

const origin = 'http://ankiquest.test';
const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=', 'base64');
interface Call { method: string; path: string; query: string; body: unknown; auth: string | undefined }
let browser: Browser | undefined;
before(async () => {
  browser = await chromium.launch({headless: true,
    ...(process.env.PLAYWRIGHT_CHANNEL ? {channel: process.env.PLAYWRIGHT_CHANNEL} : process.platform === 'win32' ? {channel: 'msedge'} : {})});
});
after(async () => browser?.close());

const win = (id: number, extra: Record<string, unknown> = {}) => ({
  id, kind: 'daily_goal', key: `goal:${id}`, day: 20391, created_at: 1793400000 - id,
  title: `Win ${id}`, body: '120 cards today in 35 minutes.', stats: {streak: 51}, art: 'celebrate',
  policy: 'ask', shared_with_friends: false, link: null, ...extra,
});

async function noOverflow(page: Page, label: string) {
  const size = await page.evaluate(() => ({width: innerWidth, scroll: document.documentElement.scrollWidth}));
  assert(size.scroll <= size.width, `${label}: ${JSON.stringify(size)}`);
}

async function personal(t: TestContext, options: {width?: number; theme?: 'light' | 'dark'; locale?: string} = {}) {
  const context = await browser!.newContext({locale: options.locale || 'en-US', viewport: {width: options.width || 390, height: 850}, colorScheme: options.theme || 'light'});
  t.after(() => context.close());
  await context.addInitScript(() => {
    Object.defineProperty(navigator, 'clipboard', {configurable: true, value: {writeText: async (text: string) => { (window as unknown as {copied: string}).copied = text; }}});
  });
  const calls: Call[] = [];
  const data = {
    profile: {user: 'alice', display: 'Alice', today: {reviews: 64, xp: 300}, streak: 5, level: 3, quests: [], goal: {mode: 'cards', value: 100, progress: 64, reached: false} as Record<string, unknown>},
    wins: [win(30, {title: 'Goal <img src=x onerror=alert(1)>'}), ...Array.from({length: 9}, (_, index) => win(29 - index)), win(5), win(4, {shared_with_friends: true, link: {url: `${origin}/w/old`, image: `${origin}/w/old.png`}})],
    sharing: {policies: {daily_goal: 'ask', streak_secured: 'private', streak: 'ask', level: 'auto', achievement: 'ask', record: 'ask', quest: 'private', conquered: 'ask', place: 'ask', challenge: 'ask'}, daily_cap: 3} as Record<string, unknown>,
    goal: {mode: 'cards', value: 100, since: 20390, chosen: true} as Record<string, unknown>,
    recipients: 2,
    rejectSharing: false,
  };
  await context.route('**/*', async route => {
    const request = route.request(), url = new URL(request.url()), pathname = url.pathname, method = request.method();
    if (['/today', '/settings'].includes(pathname)) return route.fulfill({contentType: 'text/html', body: built('personal')});
    if (await fulfillAsset(route, pathname)) return;
    if (pathname.startsWith('/api/')) calls.push({method, path: pathname, query: url.search, body: request.postDataJSON(), auth: request.headers().authorization});
    if (pathname === '/auth/status') return route.fulfill({json: {private_site: true, authenticated: true, member: {user: 'alice'}}});
    if (pathname === '/api/profile/alice') return route.fulfill({json: data.profile});
    if (pathname === '/api/study/alice') return route.fulfill({json: {year: 2026, years: [2026], days: [], last_review_at: null, last_received_at: null, latest_session: null}});
    if (pathname === '/api/activity/alice') return route.fulfill({json: {items: [], unread_count: 0}});
    if (pathname === '/api/community/challenges/alice') return route.fulfill({json: {challenges: []}});
    if (pathname === '/api/wins/alice') {
      const before = Number(url.searchParams.get('before') || Infinity), limit = Number(url.searchParams.get('limit'));
      return route.fulfill({json: data.wins.filter(item => item.id < before).slice(0, limit)});
    }
    const card = /^\/api\/wins\/alice\/(\d+)\/card\.png$/.exec(pathname);
    if (card) return route.fulfill({contentType: 'image/png', body: png});
    const link = /^\/api\/wins\/alice\/(\d+)\/link$/.exec(pathname);
    if (link && method === 'DELETE') return route.fulfill({status: 204});
    if (pathname === '/api/share/alice') {
      const body = request.postDataJSON() as {win: number; to: string};
      if (body.to === 'friends') return route.fulfill({json: {recipients: data.recipients}});
      return route.fulfill({json: {url: `${origin}/w/tok${body.win}`, image: `${origin}/w/tok${body.win}.png`, text: 'Join me'}});
    }
    if (pathname === '/api/sharing/alice') {
      if (method === 'PUT') {
        if (data.rejectSharing) return route.fulfill({status: 400, json: {error: 'daily_cap must be 1 to 10'}});
        const body = request.postDataJSON() as Record<string, unknown>;
        data.sharing = body;
      }
      return route.fulfill({json: data.sharing});
    }
    if (pathname === '/api/goal/alice') {
      if (method === 'PUT') {
        const {restart, ...body} = request.postDataJSON() as Record<string, unknown>;
        const changed = restart || body.mode !== data.goal.mode || (body.value !== undefined && body.value !== data.goal.value);
        data.goal = {...data.goal, ...body, since: changed ? 20400 : data.goal.since, chosen: true};
      }
      return route.fulfill({json: data.goal});
    }
    if (pathname === '/api/companion/alice') return route.fulfill({json: {companion: 'aki'}});
    if (pathname === '/api/community/reminders/alice') return route.fulfill({json: {gentle_daily: false, urgent_streak: false, freeze_used: false, freeze_refill: false, milestone: false, weekly_closing: false, weekly_recap: false, reminder_hour: 20, quiet_start: 22, quiet_end: 9, daily_limit: 2}});
    if (pathname === '/api/streak-freezes/alice') return route.fulfill({json: {enabled: false, freezes: 1, capacity: 3}});
    if (pathname === '/api/friend-nudges/alice') return route.fulfill({json: {receiving: true, automatic_receiving: false, friends: []}});
    if (pathname === '/api/deck-subscriptions/alice') return route.fulfill({json: {enabled: true, unsubscribed_senders: [], muted_senders: [], sharing_senders: [], senders: []}});
    if (pathname === '/api/decks/alice') return route.fulfill({json: {decks: [], recipients: [], celebrations: true}});
    if (pathname === '/manifest.webmanifest') return route.fulfill({json: {name: 'AnkiQuest'}});
    if (pathname === '/icon.svg') return route.fulfill({contentType: 'image/svg+xml', body: '<svg xmlns="http://www.w3.org/2000/svg"/>'});
    return route.fulfill({status: 404, body: ''});
  });
  const page = await context.newPage(), errors: string[] = [];
  page.setDefaultTimeout(10000);
  page.on('pageerror', error => errors.push(error.message));
  t.after(() => assert.deepEqual(errors, []));
  return {page, calls, data};
}

test('sharing settings save every moment policy and the daily cap', async t => {
  const {page, calls, data} = await personal(t);
  await page.goto(`${origin}/settings`);
  const form = page.locator('#sharing-form');
  await form.waitFor();
  assert.equal(await form.locator('.sharing-moment').count(), 10);
  for (const label of ['Daily goal', 'Streak secured', 'Streak milestones', 'Level ups', 'Achievements', 'Personal bests', 'Daily quests', 'Conquered cards', 'Top 3 on the weekly board', 'Challenges completed'])
    await form.locator('legend', {hasText: label}).first().waitFor();
  assert(await form.locator('[name="policy-level"][value="auto"]').isChecked());
  assert(await form.locator('[name="policy-quest"][value="private"]').isChecked());
  assert.equal(await page.locator('#sharing a[href="/settings#deck-sharing"]').count(), 1);
  assert.equal(await page.locator('#deck-sharing').count(), 1);
  await form.getByRole('radio', {name: 'Tell friends automatically'}).first().check();
  await form.locator('[name="policy-quest"][value="ask"]').check();
  await form.locator('[name="daily_cap"]').selectOption('7');
  await form.locator('button[type="submit"]').click();
  await page.waitForFunction(() => document.querySelector('#sharing-form .status')?.textContent?.includes('Saved'));
  const put = calls.findLast(call => call.path === '/api/sharing/alice' && call.method === 'PUT')!;
  assert.deepEqual(put.body, {policies: {daily_goal: 'auto', streak_secured: 'private', streak: 'ask', level: 'auto', achievement: 'ask', record: 'ask', quest: 'ask', conquered: 'ask', place: 'ask', challenge: 'ask'}, daily_cap: 7});
  data.rejectSharing = true;
  await page.locator('#sharing-form button[type="submit"]').click();
  await page.locator('#sharing-form .status.error', {hasText: 'daily_cap must be 1 to 10'}).waitFor();
});

test('the daily goal validates each mode and plans the backlog in days', async t => {
  const {page, calls, data} = await personal(t);
  await page.goto(`${origin}/settings#daily-goal`);
  const form = page.locator('#goal-form');
  await form.waitFor();
  assert.deepEqual(await page.locator('#goal-mode option').allTextContents(), ['Clear my backlog in N days', 'Cards per day', 'Minutes per day', 'No daily goal']);
  assert.equal(await page.locator('#goal-value').inputValue(), '100');
  assert(await page.locator('#goal-presets').isHidden());
  await page.locator('#goal-mode').selectOption('minutes');
  assert.equal(await page.locator('#goal-value').inputValue(), '20');
  assert.equal(await page.locator('#goal-value').getAttribute('max'), '600');
  await page.locator('#goal-value').fill('700');
  await form.locator('button[type="submit"]').click();
  await form.locator('.status.error', {hasText: 'Choose a whole number from 1 to 600.'}).waitFor();
  assert.equal(calls.filter(call => call.path === '/api/goal/alice' && call.method === 'PUT').length, 0);
  await page.locator('#goal-value').fill('2.5');
  await form.locator('button[type="submit"]').click();
  await form.locator('.status.error', {hasText: 'whole number'}).waitFor();
  await page.locator('#goal-value').fill('30');
  await form.locator('button[type="submit"]').click();
  await page.waitForFunction(() => document.querySelector('#goal-form .status')?.textContent?.includes('Saved'));
  assert.deepEqual(calls.findLast(call => call.path === '/api/goal/alice')!.body, {mode: 'minutes', value: 30});
  await page.locator('#goal-mode').selectOption('backlog');
  assert.match((await page.locator('#goal-hint').textContent())!, /Anki app works out each day's share/);
  assert.equal(await page.locator('#goal-value-label').textContent(), 'Days to clear the backlog');
  assert.equal(await page.locator('#goal-value').inputValue(), '30');
  assert.equal(await page.locator('#goal-value').getAttribute('max'), '365');
  assert.deepEqual(await page.locator('#goal-presets button').allTextContents(), ['7 days', '14 days', '30 days', '60 days']);
  assert.equal(await page.locator('[data-goal-days="30"]').getAttribute('aria-pressed'), 'true');
  await page.locator('[data-goal-days="14"]').click();
  assert.equal(await page.locator('#goal-value').inputValue(), '14');
  assert.equal(await page.locator('[data-goal-days="14"]').getAttribute('aria-pressed'), 'true');
  assert.equal(await page.locator('[data-goal-days="30"]').getAttribute('aria-pressed'), 'false');
  await page.locator('#goal-value').fill('366');
  await form.locator('button[type="submit"]').click();
  await form.locator('.status.error', {hasText: 'Choose a whole number from 1 to 365.'}).waitFor();
  await page.locator('#goal-value').fill('45');
  assert.equal(await page.locator('#goal-presets [aria-pressed="true"]').count(), 0);
  await form.locator('button[type="submit"]').click();
  await page.waitForFunction(() => document.querySelector('#goal-form .status')?.textContent?.includes('Saved'));
  assert.deepEqual(calls.findLast(call => call.path === '/api/goal/alice')!.body, {mode: 'backlog', value: 45});
  assert.equal(await page.locator('#goal-value').inputValue(), '45');
  const started = new Intl.DateTimeFormat('en-US', {dateStyle: 'medium', timeZone: 'UTC'}).format(new Date(20400 * 86400000));
  assert.equal(await page.locator('#goal-since').textContent(), `Plan started ${started}`);
  data.goal.since = 20380;
  await page.locator('#goal-restart').click();
  await page.waitForFunction(() => document.querySelector('#goal-form .status')?.textContent?.includes('Saved'));
  const restart = calls.findLast(call => call.path === '/api/goal/alice')!;
  assert.equal(restart.method, 'PUT');
  assert.deepEqual(restart.body, {mode: 'backlog', value: 45, restart: true});
  assert.equal(await page.locator('#goal-since').textContent(), `Plan started ${started}`);
  await page.locator('#goal-mode').selectOption('off');
  assert(await page.locator('#goal-value-field').isHidden());
  assert(await page.locator('#goal-presets').isHidden());
  await page.locator('#goal-form button[type="submit"]').click();
  await page.waitForFunction(() => document.querySelector('#goal-form .status')?.textContent?.includes('Saved'));
  const put = calls.findLast(call => call.path === '/api/goal/alice')!;
  assert.equal(put.method, 'PUT');
  assert.deepEqual(put.body, {mode: 'off'});
  assert.equal(await page.locator('#goal-since').count(), 0);
});

test('a first visit offers the default backlog goal without a plan date', async t => {
  const {page, data} = await personal(t);
  data.goal = {mode: 'backlog', value: 30, since: 20400, chosen: false};
  await page.goto(`${origin}/settings#daily-goal`);
  await page.locator('#goal-form').waitFor();
  assert.equal(await page.locator('#goal-mode').inputValue(), 'backlog');
  assert.equal(await page.locator('#goal-value').inputValue(), '30');
  assert(await page.locator('#goal-presets').isVisible());
  assert.equal(await page.locator('#goal-since').count(), 0);
});

test('Today shows the goal meter and the backlog plan', async t => {
  const f = await personal(t);
  await f.page.goto(`${origin}/today`);
  const goal = f.page.locator('#today-goal');
  await goal.waitFor();
  assert.equal(await goal.locator('progress').getAttribute('value'), '64');
  assert.equal(await goal.locator('progress').getAttribute('max'), '100');
  assert.match((await goal.textContent())!, /64 of 100 cards/);
  assert.match((await goal.textContent())!, /In progress/);
  const offset = await f.page.evaluate(() => new Date().getTimezoneOffset() * 60000);
  const today = 20400;
  Object.assign(f.data.profile, {day_ends_at: (today + 1) * 86400000 + 4 * 3600000 + offset});
  f.data.profile.goal = {mode: 'backlog', value: 30, since: today - 4, progress: null, reached: false};
  await f.page.locator('#personal-refresh').click();
  await goal.filter({hasText: 'Day 5 of 30'}).waitFor();
  assert.match((await goal.textContent())!, /Clear the backlog in 30 days/);
  assert.match((await goal.textContent())!, /In progress/);
  assert.equal(await goal.locator('progress').getAttribute('max'), '30');
  assert.equal(await goal.locator('progress').getAttribute('value'), '4');
  f.data.profile.goal = {mode: 'backlog', value: 7, since: today - 9, progress: null, reached: true};
  await f.page.locator('#personal-refresh').click();
  await goal.filter({hasText: 'Goal reached'}).waitFor();
  assert.match((await goal.textContent())!, /Day 3 of 7/);
  assert.match((await goal.textContent())!, /Today's share is done/);
  assert.equal(await goal.locator('progress').getAttribute('value'), '3');
});

test('wins share with friends, make and remove links, download and paginate', async t => {
  const {page, calls, data} = await personal(t);
  await page.goto(`${origin}/today`);
  const first = page.locator('[data-win-id="30"]');
  await first.waitFor();
  assert.equal(await page.locator('img[onerror]').count(), 0);
  assert.match((await first.locator('strong').textContent())!, /<img src=x/);
  assert.equal(await page.locator('.win-item').count(), 10);
  await page.waitForFunction(() => document.querySelector<HTMLImageElement>('[data-win-card="30"]')?.src.startsWith('blob:'));
  assert(calls.some(call => call.path === '/api/wins/alice/30/card.png'));
  assert.equal(calls.find(call => call.path === '/api/wins/alice')!.query, '?limit=10');
  await first.getByRole('button', {name: 'Share with friends'}).click();
  await first.locator('[data-win-status]', {hasText: 'Sent to 2 friends'}).waitFor();
  await first.locator('.chip', {hasText: 'Shared with friends'}).waitFor();
  assert.deepEqual(calls.findLast(call => call.path === '/api/share/alice')!.body, {win: 30, to: 'friends'});
  data.recipients = 0;
  const second = page.locator('[data-win-id="29"]');
  await second.getByRole('button', {name: 'Share with friends'}).click();
  await second.locator('[data-win-status]', {hasText: 'Nobody to tell yet'}).waitFor();
  await second.getByRole('button', {name: 'Get link'}).click();
  await second.locator('[data-win-status]', {hasText: 'Link copied.'}).waitFor();
  assert.equal(await second.locator('[data-win-url]').inputValue(), `${origin}/w/tok29`);
  assert.equal(await page.evaluate(() => (window as unknown as {copied: string}).copied), `${origin}/w/tok29`);
  await second.locator('.chip', {hasText: 'Link active'}).waitFor();
  await second.getByRole('button', {name: 'Remove link'}).click();
  assert.equal(calls.filter(call => call.method === 'DELETE').length, 0, 'removing asks once more');
  await second.getByRole('button', {name: 'Tap again to confirm'}).click();
  await second.locator('[data-win-status]', {hasText: 'Link removed.'}).waitFor();
  assert.equal(await second.locator('[data-win-url]').count(), 0);
  assert.equal(calls.findLast(call => call.method === 'DELETE')!.path, '/api/wins/alice/29/link');
  const download = page.waitForEvent('download');
  await first.getByRole('button', {name: 'Download image'}).click();
  assert.equal((await download).suggestedFilename(), 'ankiquest-win-30.png');
  await page.locator('[data-wins-more]').click();
  await page.locator('[data-win-id="4"]').waitFor();
  assert.equal(calls.findLast(call => call.path === '/api/wins/alice')!.query, '?limit=10&before=21');
  assert.equal(await page.locator('[data-wins-more]').count(), 0);
  await page.locator('[data-win-id="4"] .chip', {hasText: 'Link active'}).waitFor();
  assert.equal(await page.locator('[data-win-id="4"] [data-win-url]').inputValue(), `${origin}/w/old`);
});

test('wins show the server explanation when sharing fails', async t => {
  const {page} = await personal(t);
  await page.route(`${origin}/api/share/alice`, route => route.fulfill({status: 409, json: {error: 'This win cannot be shared yet.'}}));
  await page.goto(`${origin}/today`);
  const first = page.locator('[data-win-id="30"]');
  await first.getByRole('button', {name: 'Get link'}).click();
  await first.locator('[data-win-status].error', {hasText: 'This win cannot be shared yet.'}).waitFor();
});

test('Today and settings fit phone and desktop widths in both themes', async t => {
  for (const width of [320, 1440]) for (const theme of ['light', 'dark'] as const) {
    const {page} = await personal(t, {width, theme});
    await page.goto(`${origin}/today`);
        await page.locator('[data-win-id="29"]').getByRole('button', {name: 'Get link'}).click();
    await page.locator('[data-win-id="29"] [data-win-url]').waitFor();
    await noOverflow(page, `today ${width} ${theme}`);
    await page.goto(`${origin}/settings`);
    await page.locator('#sharing-form').waitFor();
    await noOverflow(page, `settings ${width} ${theme}`);
    const colors = await page.evaluate(() => {
      const choice = getComputedStyle(document.querySelector('.sharing-choice')!), body = getComputedStyle(document.body);
      return {choice: choice.color, body: body.backgroundColor};
    });
    assert.notEqual(colors.choice, colors.body, `${width} ${theme}: choice text is visible`);
  }
});

test('sharing settings and wins are translated', async t => {
  const {page} = await personal(t, {locale: 'es-ES'});
  await page.goto(`${origin}/settings`);
  await page.locator('#sharing-form legend', {hasText: 'Meta diaria'}).waitFor();
  await page.getByRole('radio', {name: 'Contárselo a mis amigos automáticamente'}).first().waitFor();
  await page.goto(`${origin}/today`);
  await page.getByRole('heading', {name: 'Tus logros'}).waitFor();
  await page.getByRole('button', {name: 'Compartir con amigos'}).first().waitFor();
});

async function community(t: TestContext, items: Record<string, unknown>[], width = 390, theme: 'light' | 'dark' = 'light') {
  const context = await browser!.newContext({viewport: {width, height: 844}, colorScheme: theme});
  t.after(() => context.close());
  const page = await context.newPage(), errors: string[] = [], calls: Call[] = [];
  page.setDefaultTimeout(10000);
  page.on('pageerror', error => errors.push(error.message));
  t.after(() => assert.deepEqual(errors, []));
  await page.addInitScript(() => { window.ankiquestSession = {user: 'cerro', token: 'saved-token'}; });
  const html = built('community');
  await page.route(`${origin}/**`, async route => {
    const request = route.request(), url = new URL(request.url()), pathname = url.pathname;
    if (pathname === '/community') return route.fulfill({contentType: 'text/html', body: html});
    if (await fulfillAsset(route, pathname)) return;
    if (pathname.startsWith('/w/') && pathname.endsWith('.png')) return route.fulfill({contentType: 'image/png', body: png});
    if (pathname.startsWith('/api/')) calls.push({method: request.method(), path: pathname, query: url.search, body: request.postDataJSON(), auth: request.headers().authorization});
    if (pathname === '/api/avatars') return route.fulfill({json: {}});
    if (pathname === '/auth/status') return route.fulfill({json: {private_site: false, authenticated: true, member: null}});
    if (pathname === '/api/community') return route.fulfill({json: {meta: {}, players: [{user: 'cerro', display: 'Cerro'}, {user: 'hill', display: 'Hill'}]}});
    if (pathname.startsWith('/api/friend-nudges/')) return route.fulfill({json: {receiving: true, automatic_receiving: false, friends: []}});
    if (pathname === '/api/activity/cerro') return route.fulfill({json: {items, attention: [], action_count: 0, unread_count: 0, next_before: null}});
    if (pathname === '/api/react/cerro') return route.fulfill({json: {}});
    if (pathname === '/api/reply/cerro') return route.fulfill({json: {to: 'hill'}});
    if (pathname.includes('/reminders/')) return route.fulfill({json: {gentle_daily: false, reminder_hour: 18, quiet_start: 22, quiet_end: 9, daily_limit: 3}});
    if (pathname.startsWith('/api/community/')) return route.fulfill({json: {challenges: [], recipients: []}});
    return route.fulfill({status: 404, body: ''});
  });
  await page.goto(`${origin}/community#activity`);
  return {page, calls};
}

test('activity shows shared cards, cheers and replyable reactions', async t => {
  const now = Math.floor(Date.now() / 1000);
  const items = [
    {id: 3, sender: 'hill', kind: 'share', title: 'Hill <b>reached</b> a goal', body: '“Daily goal reached!”', created_at: now, read_at: now, win: 17, image: `${origin}/w/abc123.png`},
    {id: 2, sender: 'hill', kind: 'share', title: 'Hill leveled up', body: '“Level 54”', created_at: now - 60, read_at: now, win: 18, image: 'javascript:alert(1)//.png', reacted: true},
    {id: 1, sender: 'hill', kind: 'reaction', title: '🎉 Hill cheered', body: '“Level 12”', created_at: now - 120, read_at: now},
  ];
  const {page, calls} = await community(t, items);
  const share = page.locator('.activity-item[data-notice="3"]');
  await share.waitFor();
  assert.equal(await share.locator('h3 b').count(), 0);
  const card = share.locator('a.share-card');
  assert.equal(await card.getAttribute('href'), `${origin}/w/abc123`);
  assert.equal(await card.locator('img').getAttribute('src'), `${origin}/w/abc123.png`);
  const reacted = page.locator('.activity-item[data-notice="2"]');
  assert.equal(await reacted.locator('a.share-card').count(), 0, 'only http card images are shown');
  assert(await reacted.locator('[data-cheer]').isDisabled());
  assert.equal(await reacted.locator('[data-cheer]').getAttribute('aria-pressed'), 'true');
  await share.locator('[data-cheer]').click();
  await page.locator('#activity-action-status', {hasText: 'Your cheer was sent.'}).waitFor();
  const react = calls.findLast(call => call.path === '/api/react/cerro')!;
  assert.deepEqual(react.body, {notification: 3});
  assert.equal(react.auth, 'Bearer saved-token');
  assert(await page.locator('.activity-item[data-notice="3"] [data-cheer]').isDisabled());
  const reaction = page.locator('.activity-item[data-notice="1"]');
  assert.equal(await reaction.locator('h3').textContent(), '🎉 Hill cheered: “Level 12”');
  assert.equal(await reaction.locator('[data-cheer]').count(), 0);
  await reaction.getByRole('button', {name: 'Thanks!'}).click();
  await page.locator('#activity-action-status', {hasText: 'reply was sent'}).waitFor();
  assert.deepEqual(calls.findLast(call => call.path === '/api/reply/cerro')!.body, {notification: 1, message: 'Thanks!'});
});

test('cheering shows the server explanation when it fails', async t => {
  const now = Math.floor(Date.now() / 1000);
  const {page} = await community(t, [{id: 5, sender: 'hill', kind: 'share', title: 'Hill', body: 'x', created_at: now, read_at: now, image: `${origin}/w/t.png`}]);
  await page.route(`${origin}/api/react/cerro`, route => route.fulfill({status: 404, json: {error: 'That update is gone.'}}));
  await page.locator('[data-cheer="5"]').click();
  await page.locator('#activity-action-status.error', {hasText: 'That update is gone.'}).waitFor();
  assert(await page.locator('[data-cheer="5"]').isEnabled());
});

test('shared cards fit a phone in both themes', async t => {
  const now = Math.floor(Date.now() / 1000);
  for (const theme of ['light', 'dark'] as const) {
    const {page} = await community(t, [{id: 5, sender: 'hill', kind: 'share', title: 'Hill reached a very long goal name that keeps on going and going', body: 'x', created_at: now, read_at: now, image: `${origin}/w/t.png`}], 320, theme);
    await page.locator('.share-card img').waitFor();
    await noOverflow(page, `community 320 ${theme}`);
  }
});
