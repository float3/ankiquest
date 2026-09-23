// Run: node --test tests/test_community_auth.cjs
// Set PLAYWRIGHT_MODULE if Playwright is not available on NODE_PATH.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { test, before, after } = require('node:test');
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');

const html = fs.readFileSync(path.join(__dirname, '../static/community.html'), 'utf8');
const siteScript = fs.readFileSync(path.join(__dirname, '../static/site.js'), 'utf8');
const origin = 'http://ankiquest.test';
const saved = { user: 'cerro', token: 'saved-token' };
let browser;
before(async () => {
  browser = await chromium.launch({ headless: true,
    ...(process.env.PLAYWRIGHT_CHANNEL ? { channel: process.env.PLAYWRIGHT_CHANNEL } : process.platform === 'win32' ? { channel: 'msedge' } : {}),
  });
});
after(async () => browser?.close());

async function fixture(t, session = saved, options = {}) {
  const context = await browser.newContext({ viewport: { width: 390, height: 844 } });
  t.after(() => context.close());
  const page = await context.newPage();
  page.setDefaultTimeout(10000);
  page.setDefaultNavigationTimeout(15000);
  const errors = [], requests = [];
  const control = { reject: false, hold: null, cookieUser:options.cookieUser || null };
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(session => {
    window.storageWrites = [];
    const setItem = Storage.prototype.setItem;
    Storage.prototype.setItem = function(key, value) {
      window.storageWrites.push([key, value]);
      return setItem.call(this, key, value);
    };
    if (session) window.ankiquestSession = session;
  }, session);
  await page.route(`${origin}/**`, async route => {
    const request = route.request(), url = new URL(request.url());
    if (url.pathname === '/community') return route.fulfill({ contentType: 'text/html', body: html });
    if (url.pathname === '/site.js') return route.fulfill({contentType: 'text/javascript', body: siteScript});
    if (['/avatars.js','/avatars.css','/site.css'].includes(url.pathname)) {
      const asset=path.join(__dirname,'../static',url.pathname.slice(1));
      if(fs.existsSync(asset))return route.fulfill({contentType:url.pathname.endsWith('.js')?'text/javascript':'text/css',body:fs.readFileSync(asset,'utf8')});
    }
    if(url.pathname === '/api/avatars')return route.fulfill({contentType:'application/json',body:'{}'});
    if (url.pathname === '/auth/status') return route.fulfill({contentType: 'application/json', body: JSON.stringify({private_site:false, authenticated:true, member:control.cookieUser ? {user:control.cookieUser} : null})});
    if (url.pathname === '/auth/session' || url.pathname === '/auth/logout') return route.fulfill({status:404, body:''});
    let data;
    if (url.pathname === '/api/community') data = { meta: {}, players: [{ user: 'cerro', display: 'Cerro' }, { user: 'hill', display: 'Hill' }] };
    else if (url.pathname.startsWith('/api/activity/')) {
      data = {items:[{id:1,sender:'hill',kind:'message',title:'Saved encouragement',body:'Nice studying!',created_at:Math.floor(Date.now()/1000),read_at:null}], unread_count:1,next_before:null};
    }
    else if (url.pathname.startsWith('/api/community/')) {
      requests.push({ url: url.pathname, auth: request.headers().authorization, csrf: request.headers()['x-ankiquest-csrf'], method: request.method(), body: request.postDataJSON() });
      if (control.hold) await control.hold;
      const cookieAuthorized=!request.headers().authorization && control.cookieUser===decodeURIComponent(url.pathname.split('/').pop());
      if(cookieAuthorized && request.method()==='POST' && request.headers()['x-ankiquest-csrf']!=='1')return route.fulfill({status:403,body:'{}'});
      if (control.reject || (!cookieAuthorized && !['Bearer saved-token', 'Bearer hill-token'].includes(request.headers().authorization))) return route.fulfill({ status: 401, body: '{}' });
      data = url.pathname.includes('/reminders/')
        ? { gentle_daily: false, reminder_hour: 18, quiet_start: 22, quiet_end: 9, daily_limit: 3, ...(request.method() === 'POST' ? request.postDataJSON() : {}) }
        : { challenges: [], recipients: [] };
    } else return route.fulfill({ status: 404, body: '' });
    await route.fulfill({ contentType: 'application/json', body: JSON.stringify(data) });
  });
  await page.goto(`${origin}/community#reminders`);
  await page.waitForFunction(() => document.querySelector('#community-app').getAttribute('aria-busy') === 'false');
  t.after(() => assert.deepEqual(errors, []));
  return { page, requests, control,
    async deliver(value = saved) {
      await page.evaluate(value => { window.ankiquestSession = value; window.dispatchEvent(new CustomEvent('ankiquest-auth')); }, value);
    },
  };
}

test('community reminders reuse the saved account, including saves, without persisting its token', async t => {
  const { page, requests } = await fixture(t);
  await page.locator('#reminder-form').waitFor();
  assert.equal(await page.locator('#auth-dialog').evaluate(dialog => dialog.open), false);
  await page.locator('[name=gentle_daily]').check();
  await page.getByRole('button', { name: 'Save reminder preferences' }).click();
  await page.getByText('Your reminder preferences are saved.').waitFor();
  assert.ok(requests.every(request => request.auth === 'Bearer saved-token' && request.url.endsWith('/cerro')));
  assert.equal(requests.find(request => request.method === 'POST').body.gentle_daily, true);
  const persisted = await page.evaluate(() => JSON.stringify([localStorage, sessionStorage, window.storageWrites, location.href]));
  assert.ok(!persisted.includes(saved.token));
});

test('late native credentials connect, while explicit disconnect survives repeated auth events', async t => {
  const fixtureResult = await fixture(t, null);
  const { page, requests, deliver } = fixtureResult;
  assert.equal(requests.length, 0);
  await deliver();
  await page.locator('#reminder-form').waitFor();
  await page.locator('#view-reminders [data-disconnect]').click();
  await deliver();
  await page.locator('#reminder-form').waitFor({ state: 'detached' });
  await page.locator('#connect-button').click();
  await page.locator('#reminder-form').waitFor();
  assert.equal(await page.locator('#auth-dialog').evaluate(dialog => dialog.open), false);
});

test('standalone visitors can connect a different player with their own token', async t => {
  const { page, requests } = await fixture(t, null);
  await page.locator('#connect-button').click();
  await page.locator('#auth-user').selectOption('hill');
  await page.locator('#auth-token').fill('hill-token');
  await page.locator('#auth-submit').click();
  await page.locator('#reminder-form').waitFor();
  assert.ok(requests.every(request => request.url.endsWith('/hill') && request.auth === 'Bearer hill-token'));
});

test('an account change ignores the old response and never mixes account credentials', async t => {
  const { page, requests, control, deliver } = await fixture(t, null);
  let release;
  control.hold = new Promise(resolve => { release = resolve; });
  await deliver();
  await page.locator('#view-reminders').getByText('Loading your account…').waitFor();
  await deliver({ user: 'hill', token: 'hill-token' });
  control.hold = null;
  release();
  await page.locator('#reminder-form').waitFor();
  assert.equal(await page.locator('#view-reminders .auth-status strong').innerText(), 'Hill');
  assert.ok(requests.every(request => request.auth === (request.url.endsWith('/cerro') ? 'Bearer saved-token' : 'Bearer hill-token')));
});

test('removing the native account clears private settings and ignores in-flight results', async t => {
  const { page, control, deliver } = await fixture(t, null);
  let release;
  control.hold = new Promise(resolve => { release = resolve; });
  await deliver();
  await page.locator('#view-reminders').getByText('Loading your account…').waitFor();
  await deliver(null);
  release();
  control.hold = null;
  await page.locator('#view-reminders [data-connect]').waitFor();
  assert.equal(await page.locator('#reminder-form').count(), 0);
  assert.equal(await page.locator('#view-reminders .auth-status').count(), 0);
});

test('a rejected saved token returns to manual connection and is not automatically retried', async t => {
  const { page, requests, control, deliver } = await fixture(t, null);
  control.reject = true;
  await deliver();
  await page.locator('#view-reminders [role=alert]').waitFor();
  const count = requests.length;
  await deliver();
  await page.locator('#view-reminders [role=alert]').waitFor();
  assert.equal(requests.length, count);
  control.reject = false;
  await page.locator('#connect-button').click();
  await page.locator('#reminder-form').waitFor();
});

test('cookie account restores reminders and Activity and sends CSRF-protected saves', async t => {
  const {page,requests}=await fixture(t,null,{cookieUser:'cerro'});
  await page.locator('#reminder-form').waitFor();
  await page.locator('[name=gentle_daily]').check();
  await page.getByRole('button',{name:'Save reminder preferences'}).click();
  await page.getByText('Your reminder preferences are saved.').waitFor();
  assert(requests.every(request=>!request.auth));
  assert.equal(requests.find(request=>request.method==='POST').csrf,'1');
  await page.locator('#tab-activity').click();
  await page.getByText('Saved encouragement').waitFor();
});
test('cookie account replacement clears Activity and uses the new owner for private settings', async t => {
  const {page,requests,control}=await fixture(t,null,{cookieUser:'cerro'});
  await page.locator('#reminder-form').waitFor();
  control.cookieUser='hill';await page.evaluate(()=>AnkiQuestSite.status(true));
  await page.locator('#view-reminders .auth-status strong').filter({hasText:'Hill'}).waitFor();
  assert(requests.filter(request=>request.url.endsWith('/hill')).length>=2);
  assert(requests.every(request=>!request.auth));
  control.cookieUser=null;await page.evaluate(()=>AnkiQuestSite.status(true));
  await page.locator('#view-reminders [data-connect]').waitFor();
  assert.equal(await page.locator('.activity-item').count(),0);
});

test('a removed native account is not restored from its old browser cookie on page restoration', async t=>{
  const {page,deliver}=await fixture(t,saved,{cookieUser:'cerro'});
  await page.locator('#reminder-form').waitFor();await deliver(null);
  await page.locator('#view-reminders [data-connect]').waitFor();
  await page.evaluate(async()=>{dispatchEvent(new PageTransitionEvent('pageshow',{persisted:true}));await AnkiQuestSite.status(true);await new Promise(resolve=>setTimeout(resolve,200));});
  assert.equal(await page.locator('#reminder-form').count(),0);
  assert.equal(await page.locator('.activity-item').count(),0);
});

test('a first null native event clears private Community views already loaded with a cookie', async t=>{
  const {page,deliver}=await fixture(t,null,{cookieUser:'cerro'});
  await page.locator('#reminder-form').waitFor();await deliver(null);
  assert.equal(await page.locator('#reminder-form').count(),0);
  assert.equal(await page.locator('.activity-item').count(),0);
  await page.locator('#view-reminders [data-connect]').waitFor();
});
