import assert from 'node:assert/strict';
import {test} from 'node:test';
import type {Browser} from 'playwright';
import {chromium, fulfillAsset, page as built} from './support/web.ts';

async function login(browser: Browser, oauth: string[] | undefined) {
  const context = await browser.newContext({locale: 'en-US', viewport: {width: 390, height: 900}});
  await context.route('**/*', async route => {
    const pathname = new URL(route.request().url()).pathname;
    if (pathname === '/login') return route.fulfill({contentType: 'text/html', body: built('login')});
    if (await fulfillAsset(route, pathname)) return;
    if (pathname === '/auth/status') return route.fulfill({json: {private_site: false, registration: true, authenticated: false, member: null, ...(oauth ? {oauth} : {})}});
    return route.fulfill({status: 404, body: ''});
  });
  const page = await context.newPage(), errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto('http://ankiquest.test/login?next=%2Ftoday');
  await page.locator('#accounts').waitFor();
  await page.waitForTimeout(100);
  const links = await page.locator('#oauth a').evaluateAll(links => links.map(link => [link.textContent, link.getAttribute('href')]));
  const shown = await page.locator('#oauth').isVisible();
  await context.close();
  return {links, shown, errors};
}

async function settings(browser: Browser) {
  const context = await browser.newContext({locale: 'en-US', viewport: {width: 390, height: 1400}});
  const posted: unknown[] = [];
  const linked = true;
  await context.route('**/*', async route => {
    const request = route.request(), pathname = new URL(request.url()).pathname;
    if (pathname === '/settings') return route.fulfill({contentType: 'text/html', body: built('personal')});
    if (await fulfillAsset(route, pathname)) return;
    if (pathname === '/auth/status') return route.fulfill({json: {private_site: false, registration: true, authenticated: true, member: {user: 'ana', display: 'Ana'}, oauth: ['github', 'gitlab']}});
    if (pathname === '/api/accounts/ana') return route.fulfill({json: {user: 'ana', display: 'Ana', created_at: 0, password: false}});
    if (pathname === '/api/sign-in/ana') {
      if (request.method() === 'POST') {
        const body = request.postDataJSON();
        posted.push({csrf: request.headers()['x-ankiquest-csrf'] ?? null, body});
        if (body.action === 'token') return route.fulfill({status: 201, json: {token: 'fresh-device-token'}});
      }
      return route.fulfill({json: {account: true, password: false, providers: [
        {provider: 'github', name: 'GitHub', linked: false},
        {provider: 'gitlab', name: 'GitLab', linked},
      ]}});
    }
    return route.fulfill({status: 404, body: ''});
  });
  const page = await context.newPage(), errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto('http://ankiquest.test/settings');
  await page.locator('.sign-in-row').first().waitFor();
  const result = {
    rows: (await page.locator('.sign-in-row').allInnerTexts()).map(text => text.replace(/\s+/g, ' ')),
    link: await page.locator('.sign-in-row a').getAttribute('href'),
    unlinkDisabled: await page.locator('[data-unlink=gitlab]').isDisabled(),
    confirmField: await page.locator('#delete-account-form input[name=confirm]').count(),
    passwordField: await page.locator('#delete-account-form input[name=password]').count(),
    token: '',
    posted,
    wide: await page.evaluate(() => document.documentElement.scrollWidth > innerWidth),
    errors,
  };
  await page.click('#new-device-token');
  await page.locator('#device-token', {hasText: 'fresh-device-token'}).waitFor();
  result.token = (await page.locator('#device-token').textContent()) ?? '';
  await context.close();
  return result;
}

test('sign-in buttons appear only for configured providers', async () => {
  const browser = await chromium.launch({headless: true, channel: process.env.PLAYWRIGHT_CHANNEL || 'msedge'});
  try {
    const configured = await login(browser, ['github', 'gitlab', 'unknown']);
    assert.equal(configured.shown, true);
    assert.deepEqual(configured.links, [
      ['Sign in with GitHub', '/auth/oauth/github?next=%2Ftoday'],
      ['Sign in with GitLab', '/auth/oauth/gitlab?next=%2Ftoday'],
    ]);
    assert.deepEqual(configured.errors, []);
    const none = await login(browser, undefined);
    assert.equal(none.shown, false);
    assert.deepEqual(none.links, []);
    assert.deepEqual(none.errors, []);
  } finally {
    await browser.close();
  }
});

test('settings link providers, keep the last sign-in and make device tokens', async () => {
  const browser = await chromium.launch({headless: true, channel: process.env.PLAYWRIGHT_CHANNEL || 'msedge'});
  try {
    const page = await settings(browser);
    assert.deepEqual(page.rows, ['GitHub Link', 'GitLab Unlink']);
    assert.equal(page.link, '/auth/oauth/github?link=1');
    assert.equal(page.unlinkDisabled, true, 'the only way in cannot be unlinked');
    assert.equal(page.confirmField, 1, 'an account without a password confirms deletion with its username');
    assert.equal(page.passwordField, 0);
    assert.equal(page.token, 'fresh-device-token');
    assert.deepEqual(page.posted, [{csrf: '1', body: {action: 'token'}}]);
    assert.equal(page.wide, false);
    assert.deepEqual(page.errors, []);
  } finally {
    await browser.close();
  }
});
