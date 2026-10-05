// The leaderboard welcomes visitors without an account, but only on servers that take sign-ups.
import assert from 'node:assert/strict';
import {test} from 'node:test';
import type {Browser} from 'playwright';
import {chromium, fulfillAsset, page as built} from './support/web.ts';

async function visit(browser: Browser, access: Record<string, unknown>) {
  const context = await browser.newContext({locale: 'en-US', viewport: {width: 390, height: 850}});
  await context.route('**/*', async route => {
    const pathname = new URL(route.request().url()).pathname;
    if (pathname === '/week') return route.fulfill({contentType: 'text/html', body: built('index')});
    if (await fulfillAsset(route, pathname)) return;
    if (pathname === '/auth/status') return route.fulfill({json: access});
    if (pathname === '/api/leaderboard') return route.fulfill({json: []});
    if (pathname === '/api/social/alice') return route.fulfill({json: {groups: [], friends: []}});
    if (pathname === '/api/week') return route.fulfill({json: {ends_at: Date.now() + 86400000, timezone: 'UTC'}});
    if (pathname === '/api/winners') return route.fulfill({json: {}});
    if (pathname === '/manifest.webmanifest') return route.fulfill({json: {name: 'AnkiQuest'}});
    if (pathname === '/icon.svg') return route.fulfill({contentType: 'image/svg+xml', body: '<svg xmlns="http://www.w3.org/2000/svg"/>'});
    return route.fulfill({status: 404, body: pathname});
  });
  const page = await context.newPage(), errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto('http://ankiquest.test/week');
  await page.locator('.card-head h2').first().waitFor();
  const result = {
    welcome: await page.locator('.welcome').count(),
    signIn: await page.locator('.site-signin').isVisible(),
    signUp: await page.locator('.welcome a.primary').getAttribute('href').catch(() => null),
    errors,
  };
  await context.close();
  return result;
}

test('newcomers see what AnkiQuest is and how to start; members and private communities do not', async () => {
  const browser = await chromium.launch({headless: true, channel: process.env.PLAYWRIGHT_CHANNEL || 'msedge'});
  try {
    const newcomer = await visit(browser, {private_site: false, authenticated: false, registration: true, member: null});
    assert.deepEqual(newcomer, {welcome: 1, signIn: true, signUp: '/login?signup=1', errors: []});
    const member = await visit(browser, {private_site: false, authenticated: true, registration: true, member: {user: 'alice'}});
    assert.deepEqual(member, {welcome: 0, signIn: false, signUp: null, errors: []});
    const closed = await visit(browser, {private_site: false, authenticated: false, registration: false, member: null});
    assert.deepEqual(closed, {welcome: 0, signIn: false, signUp: null, errors: []});
  } finally {
    await browser.close();
  }
});
