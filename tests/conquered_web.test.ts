// A conquered card's page: a big companion card anyone allowed can see, and telling friends for its owner.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {test} from 'node:test';
import type {Browser} from 'playwright';
import {chromium, fulfillAsset, page as built, root} from './support/web.ts';

const AT = 1789000000456;

async function visit(browser: Browser, owner: boolean, leech: boolean, shot = '') {
  const context = await browser.newContext({locale: 'en-US', viewport: {width: 390, height: 1000}});
  const told: {csrf: string | null}[] = [];
  await context.route('**/*', async route => {
    const request = route.request(), pathname = new URL(request.url()).pathname;
    if (pathname === `/conquered/hill/${AT}`) return route.fulfill({contentType: 'text/html', body: built('conquered')});
    if (await fulfillAsset(route, pathname)) return;
    if (/^\/(aki|ankilope)\/\w+\.png$/.test(pathname)) return route.fulfill({contentType: 'image/png', body: fs.readFileSync(path.join(root, 'static', pathname))});
    if (pathname === '/auth/status') return route.fulfill({json: {private_site: false, authenticated: owner, member: owner ? {user: 'hill'} : null}});
    if (pathname === `/api/conquests/hill/${AT}`) {
      const conquest = {at: AT, lapses: leech ? 9 : 4, answers: 23, since: AT - 40 * 86_400_000, leech};
      return route.fulfill({json: {user: 'hill', display: 'Hill', conquest, ...(owner ? {told: false, friends: 2} : {})}});
    }
    if (pathname === `/api/conquests/hill/${AT}/tell` && request.method() === 'POST') {
      told.push({csrf: request.headers()['x-ankiquest-csrf'] ?? null});
      return route.fulfill({json: {told: 2}});
    }
    return route.fulfill({status: 404, body: pathname});
  });
  const page = await context.newPage(), errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(`http://ankiquest.test/conquered/hill/${AT}`);
  await page.locator('.conquest h1').waitFor();
  if (shot) await page.screenshot({path: shot, fullPage: true});
  const result = {
    heading: await page.locator('.conquest h1').textContent(),
    tamed: await page.locator('.conquest.tamed').count(),
    pose: await page.locator('.conquest-hero .aki-art').getAttribute('data-companion-pose'),
    tell: await page.locator('#conquest-tell').count(),
    told,
    status: '',
    errors,
  };
  if (result.tell) {
    await page.locator('#conquest-tell').click();
    await page.locator('#conquest-status', {hasText: 'activity'}).waitFor();
    result.status = (await page.locator('#conquest-status').textContent()) ?? '';
  }
  await context.close();
  return result;
}

test('a conquered card is a big companion card; only its owner can tell friends', async () => {
  const browser = await chromium.launch({headless: true, channel: process.env.PLAYWRIGHT_CHANNEL || 'msedge'});
  const shots = process.env.CONQUERED_SCREENSHOTS;
  try {
    const mine = await visit(browser, true, false, shots ? path.join(shots, 'conquered.png') : '');
    assert.deepEqual(mine, {heading: 'Card conquered!', tamed: 0, pose: 'celebrate', tell: 1, told: [{csrf: '1'}], status: '2 friends will see it in their activity.', errors: []});
    const theirs = await visit(browser, false, true, shots ? path.join(shots, 'tamed.png') : '');
    assert.deepEqual(theirs, {heading: 'Leech tamed!', tamed: 1, pose: 'winner', tell: 0, told: [], status: '', errors: []});
  } finally {
    await browser.close();
  }
});
