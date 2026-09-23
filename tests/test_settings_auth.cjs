// Run: node --test tests/test_settings_auth.cjs
// Point PLAYWRIGHT_MODULE at an installed Playwright package if it is not on NODE_PATH.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { test, before, after } = require('node:test');
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');

const html = fs.readFileSync(path.join(__dirname, '../static/index.html'), 'utf8');
const siteScript = fs.readFileSync(path.join(__dirname, '../static/site.js'), 'utf8');
const origin = 'http://ankiquest.test';
const savedSession = { user: 'cerro', token: 'saved-token' };
const dialogs = [
  { name: 'freezes', open: '#manage-freezes', id: '#streak-freezes', ready: '#freeze-preferences', input: '#freeze-token', unlock: '#freeze-unlock', save: 'Save preference', endpoint: 'streak-freezes' },
  { name: 'decks', open: '#manage-decks', id: '#deck-sharing', ready: '#deck-settings', input: '#deck-token', unlock: '#deck-unlock', save: 'Save preferences', endpoint: 'decks' },
];
const incoming = { name:'incoming', open:'#manage-received-notifications', id:'#notification-preferences', ready:'#notification-settings', input:'#notification-token', unlock:'#notification-unlock', save:'Save preferences', endpoint:'deck-subscriptions' };
let browser;
before(async () => {
  browser = await chromium.launch({
    headless: true,
    ...(process.env.PLAYWRIGHT_CHANNEL ? { channel: process.env.PLAYWRIGHT_CHANNEL } : process.platform === 'win32' ? { channel: 'msedge' } : {}),
  });
});
after(async () => browser?.close());

async function fixture(t, session = savedSession, user = 'cerro', options = {}) {
  const context = await browser.newContext({ viewport: { width: 390, height: 844 }, colorScheme: 'dark' });
  const pendingReleases = [];
  t.after(async () => {
    pendingReleases.forEach(release => release());
    await context.close();
  });
  const page = await context.newPage();
  page.setDefaultTimeout(10000);
  page.setDefaultNavigationTimeout(15000);
  const errors = [], requests = [];
  const state = { nextFailure: null, acceptedToken: 'saved-token', cookieUser: options.cookieUser || null };
  page.on('pageerror', error => errors.push(error.message));
  page.on('request', request => requests.push({ url: request.url(), method: request.method(), auth: request.headers().authorization, csrf: request.headers()['x-ankiquest-csrf'], body: request.postData() }));
  await page.addInitScript(session => {
    window.authStorageWrites = [];
    window.settingsFetchSignals = [];
    const fetch = window.fetch;
    window.fetch = function (input, options) {
      if (/^\/api\/(streak-freezes|decks|deck-subscriptions)\//.test(String(input))) window.settingsFetchSignals.push(options.signal);
      return fetch.call(this, input, options);
    };
    const setItem = Storage.prototype.setItem;
    Storage.prototype.setItem = function (key, value) {
      window.authStorageWrites.push([String(key), String(value)]);
      return setItem.call(this, key, value);
    };
    if (session) {
      window.ankiquestSession = session;
      window.dispatchEvent(new CustomEvent('ankiquest-auth'));
    }
  }, session);
  await page.route(`${origin}/**`, async route => {
    const request = route.request(), url = new URL(request.url());
    if (url.pathname === '/site.js') return route.fulfill({contentType: 'text/javascript', body: siteScript});
    if (['/avatars.js','/avatars.css','/site.css'].includes(url.pathname)) {
      const asset=path.join(__dirname,'../static',url.pathname.slice(1));
      if(fs.existsSync(asset))return route.fulfill({contentType:url.pathname.endsWith('.js')?'text/javascript':'text/css',body:fs.readFileSync(asset,'utf8')});
    }
    if(url.pathname === '/api/avatars')return route.fulfill({contentType:'application/json',body:'{}'});
    if (url.pathname === '/auth/status') return route.fulfill({contentType: 'application/json', body: JSON.stringify({private_site:false, authenticated:true, member:state.cookieUser ? {user:state.cookieUser} : null})});
    if (url.pathname === '/auth/session' || url.pathname === '/auth/logout') {
      if (!options.modern) return route.fulfill({status:404, body:''});
      if(url.pathname === '/auth/session' && state.holdSession) {
        const held=state.holdSession;state.holdSession=null;held.arrive();await held.released;
      }
      state.cookieUser = url.pathname === '/auth/logout' ? null : request.headers().authorization === 'Bearer hill-token' ? 'hill' : 'cerro';
      return route.fulfill({status:204});
    }
    let data;
    if (url.pathname === '/') return route.fulfill({ contentType: 'text/html', body: html });
    if (url.pathname.startsWith('/api/community/reminders/')) return route.fulfill({status: request.headers().authorization === `Bearer ${state.acceptedToken}` ? 200 : 401, contentType:'application/json',body:'{}'});
    if (url.pathname === '/api/leaderboard') data = [];
    else if (url.pathname === '/api/week') data = {};
    else if (url.pathname.startsWith('/api/profile/')) {
      const profileUser = decodeURIComponent(url.pathname.slice('/api/profile/'.length));
      data = {
        user: profileUser, display: profileUser, level: 1, xp_into_level: 0, xp_for_next: 100, xp_total: 0,
        streak: 1, freezes: 0, stored_freezes: 2, freezes_enabled: false,
        today: { reviews: 0, xp: 0 }, quests: [], heatmap: [{ date: '2026-09-22', reviews: 0, xp: 0 }],
        lifetime: { reviews: 0, hours: 0, best_streak: 1, best_combo: 0, days_active: 1 }, achievements: [],
      };
    } else if (/^\/api\/(streak-freezes|decks|deck-subscriptions)\//.test(url.pathname)) {
      let forcedStatus;
      if (state.holdNext) {
        const held = state.holdNext;
        state.holdNext = null;
        held.arrive();
        await held.released;
        forcedStatus = held.status;
      }
      if (state.nextFailure) {
        const failure = state.nextFailure;
        state.nextFailure = null;
        if (failure === 'network') return route.abort('failed');
        return route.fulfill({ status: failure, body: '' });
      }
      const cookieAuthorized = !request.headers().authorization && state.cookieUser === decodeURIComponent(url.pathname.split('/').pop());
      if (cookieAuthorized && request.method() === 'POST' && request.headers()['x-ankiquest-csrf'] !== '1') return route.fulfill({status:403,body:''});
      if (forcedStatus === 401 || (forcedStatus !== 200 && !cookieAuthorized && request.headers().authorization !== `Bearer ${state.acceptedToken}`)) return route.fulfill({ status: 401, body: '' });
      data = url.pathname.startsWith('/api/streak-freezes/')
        ? { enabled: request.method() === 'POST' ? request.postDataJSON().enabled : false, freezes: 2, capacity: 3 }
        : url.pathname.startsWith('/api/deck-subscriptions/')
        ? { enabled: request.method() === 'POST' ? request.postDataJSON().enabled : true, muted_senders:request.method() === 'POST' ? [] : options.muted || [], unsubscribed_senders:request.method() === 'POST' ? request.postDataJSON().unsubscribed_senders : options.unsubscribed || [], sharing_senders:options.sharing || ['hill'], senders:[{user:'hill',display:'Hill'}] }
        : { decks: [{ id: '42', name: 'Spanish', enabled: false, recipients: [] }], recipients: [], nudges: false };
    } else return route.fulfill({ status: 404, body: '' });
    return route.fulfill({ contentType: 'application/json', body: JSON.stringify(data) });
  });
  await page.goto(`${origin}/#${user}`);
  await page.locator('#manage-freezes').waitFor();
  t.after(() => assert.deepEqual(errors, [], 'no uncaught browser errors'));
  return {
    page, state,
    holdNext(status) {
      let arrive, release;
      const arrived = new Promise(resolve => { arrive = resolve; });
      const released = new Promise(resolve => { release = resolve; });
      state.holdNext = { arrive, released, status };
      pendingReleases.push(release);
      return { arrived, release };
    },
    holdSession() {
      let arrive,release;
      const arrived=new Promise(resolve=>{arrive=resolve;}),released=new Promise(resolve=>{release=resolve;});
      state.holdSession={arrive,released};pendingReleases.push(release);return{arrived,release};
    },
    settingsRequests: () => requests.filter(request => /\/api\/(streak-freezes|decks|deck-subscriptions)\//.test(request.url)),
    async deliver(session = savedSession) {
      await page.evaluate(value => {
        window.ankiquestSession = value;
        window.dispatchEvent(new CustomEvent('ankiquest-auth'));
      }, session);
    },
    async open(spec) { await page.locator(spec.open).click(); },
    async ready(spec) { await page.locator(spec.ready).waitFor(); },
    async close(spec) {
      await page.locator(spec.id).getByRole('button', { name: 'Close', exact: true }).click();
      await page.waitForFunction(id => !document.querySelector(id).open && document.querySelector(id).innerHTML === '', spec.id);
    },
    async unlock(spec, token = state.acceptedToken) {
      await page.locator(spec.input).fill(token);
      await page.locator(spec.unlock).getByRole('button').click();
      await page.locator(spec.ready).waitFor();
    },
    async noCredentialPersistence(...tokens) {
      const contents = await page.evaluate(() => ({
        storage: JSON.stringify([localStorage, sessionStorage, window.authStorageWrites]),
        url: location.href,
      }));
      for (const token of tokens) {
        assert.ok(!contents.storage.includes(token), 'credentials must never enter browser storage');
        assert.ok(!contents.url.includes(token), 'credentials must never enter the page URL');
        assert.ok(requests.every(request => !request.url.includes(token)), 'credentials must never enter request URLs');
        assert.ok(requests.every(request => !request.body?.includes(token)), 'credentials must only be sent in the authorization header');
      }
    },
  };
}

test('decks: an unexpected response can be retried without reopening', async t => {
  const f = await fixture(t);
  const spec = dialogs.find(dialog => dialog.name === 'decks');
  await f.page.route(`${origin}/api/decks/**`, route => route.fulfill({
    contentType: 'application/json', body: JSON.stringify({ decks: null, recipients: [], nudges: false }),
  }), { times: 1 });
  await f.open(spec);
  await f.page.locator(spec.id).getByRole('status').filter({ hasText: /sort|unexpected|try again/i }).waitFor();
  const retry = f.page.locator(spec.unlock).getByRole('button');
  assert.equal(await retry.isEnabled(), true);
  await retry.click();
  await f.ready(spec);
  assert.equal(f.settingsRequests().length, 2);
  assert.ok(f.settingsRequests().every(request => request.auth === 'Bearer saved-token'));
});

for (const spec of dialogs) {
  test(`${spec.name}: saved native credentials automatically load settings before opening`, async t => {
    const f = await fixture(t);
    await f.open(spec);
    await f.ready(spec);
    assert.equal(await f.page.locator(spec.input).count(), 0, 'an authenticated player must not need to reenter the token');
    assert.equal(f.settingsRequests().length, 1);
    assert.equal(f.settingsRequests()[0].auth, 'Bearer saved-token');
    assert.equal(f.settingsRequests()[0].method, 'GET', 'opening settings must not change preferences');
    assert.equal(await f.page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, 'mobile settings must fit the viewport');
    if (process.env.SETTINGS_SCREENSHOT_DIR) {
      fs.mkdirSync(process.env.SETTINGS_SCREENSHOT_DIR, { recursive: true });
      await f.page.screenshot({ path: path.join(process.env.SETTINGS_SCREENSHOT_DIR, `settings-${spec.name}-mobile.png`) });
    }
    await f.close(spec);
    await f.open(spec);
    await f.ready(spec);
    await f.close(spec);
    const other = dialogs.find(candidate => candidate !== spec);
    await f.open(other);
    await f.ready(other);
    assert.equal(f.settingsRequests().length, 3);
    assert.ok(f.settingsRequests().every(request => request.auth === 'Bearer saved-token' && request.method === 'GET'));
    await f.noCredentialPersistence('saved-token');
  });

  test(`${spec.name}: native credentials arriving after opening unlock the dialog`, async t => {
    const f = await fixture(t, null);
    await f.open(spec);
    await f.page.locator(spec.input).waitFor();
    await f.deliver();
    await f.ready(spec);
    assert.equal(f.settingsRequests().length, 1);
    await f.noCredentialPersistence('saved-token');
  });

  test(`${spec.name}: duplicate auth events do not reload settings`, async t => {
    const f = await fixture(t, null);
    const held = f.holdNext();
    await f.open(spec);
    await f.deliver();
    await held.arrived;
    await f.deliver();
    await f.deliver();
    held.release();
    await f.ready(spec);
    const preference = f.page.locator(spec.name === 'freezes' ? '#freeze-enabled' : '#nudge-enabled');
    await preference.check();
    await f.deliver();
    await f.page.evaluate(() => new Promise(resolve => setTimeout(resolve, 50)));
    assert.equal(f.settingsRequests().length, 1, 'auth events while loading or already unlocked must not start another request');
    assert.equal(await preference.isChecked(), true, 'duplicate credentials must preserve unsaved preferences');
  });

  test(`${spec.name}: closing a pending dialog aborts the request and removes its auth listener`, async t => {
    const f = await fixture(t);
    const held = f.holdNext();
    await f.open(spec);
    await held.arrived;
    await f.close(spec);
    assert.equal(await f.page.evaluate(() => window.settingsFetchSignals.at(-1).aborted), true);
    held.release();
    await f.deliver();
    await f.page.evaluate(() => new Promise(resolve => setTimeout(resolve, 50)));
    assert.equal(f.settingsRequests().length, 1, 'the closed dialog must not handle late auth delivery');
    assert.equal(await f.page.locator(spec.id).innerHTML(), '');
    await f.open(spec);
    await f.ready(spec);
    assert.equal(f.settingsRequests().length, 2);
  });

  test(`${spec.name}: a public browser can enter a token once and reuse it in both dialogs`, async t => {
    const f = await fixture(t, null);
    await f.open(spec);
    assert.equal(f.settingsRequests().length, 0);
    await f.unlock(spec);
    await f.close(spec);
    await f.open(spec);
    await f.ready(spec);
    await f.close(spec);
    const other = dialogs.find(candidate => candidate !== spec);
    await f.open(other);
    await f.ready(other);
    await f.close(other);
    await f.noCredentialPersistence('saved-token');
    await f.page.reload();
    await f.open(spec);
    await f.page.locator(spec.input).waitFor();
    assert.equal(await f.page.locator(spec.input).inputValue(), '', 'a page reload must discard manually supplied credentials');
  });

  test(`${spec.name}: viewing another player never reuses the configured player's token`, async t => {
    const f = await fixture(t, savedSession, 'other');
    await f.open(spec);
    await f.page.locator(spec.input).waitFor();
    await f.deliver();
    await f.page.evaluate(() => new Promise(resolve => setTimeout(resolve, 50)));
    assert.equal(f.settingsRequests().length, 0);
    assert.equal(await f.page.locator(spec.input).inputValue(), '');
    await f.noCredentialPersistence('saved-token');
  });

  test(`${spec.name}: malformed native credentials leave a working manual fallback`, async t => {
    const f = await fixture(t, { user: 'cerro', token: 42 });
    await f.open(spec);
    await f.page.locator(spec.input).waitFor();
    assert.equal(f.settingsRequests().length, 0);
    await f.unlock(spec);
    await f.close(spec);
    await f.open(spec);
    await f.ready(spec);
    assert.equal(f.settingsRequests().length, 2);
  });

  test(`${spec.name}: removing the native account also removes its credentials for later dialogs`, async t => {
    const f = await fixture(t);
    await f.open(spec);
    await f.ready(spec);
    await f.close(spec);
    await f.deliver(null);
    await f.open(spec);
    await f.page.locator(spec.input).waitFor();
    assert.equal(await f.page.locator(spec.input).inputValue(), '');
    await f.close(spec);
    const other = dialogs.find(candidate => candidate !== spec);
    await f.open(other);
    await f.page.locator(other.input).waitFor();
    assert.equal(f.settingsRequests().length, 1, 'a cleared native credential must not survive in a second memory cache');
  });

  test(`${spec.name}: a pending native success cannot restore a removed account`, async t => {
    const f = await fixture(t);
    const held = f.holdNext(200);
    await f.open(spec);
    await held.arrived;
    await f.deliver(null);
    held.release();
    await f.page.locator(spec.input).waitFor();
    await f.close(spec);
    await f.open(spec);
    await f.page.locator(spec.input).waitFor();
    assert.equal(f.settingsRequests().length, 1);
  });

  for (const oldStatus of [200, 401]) {
    test(`${spec.name}: a pending ${oldStatus} response cannot replace newer native credentials`, async t => {
      const f = await fixture(t);
      const held = f.holdNext(oldStatus);
      await f.open(spec);
      await held.arrived;
      f.state.acceptedToken = 'replacement-token';
      await f.deliver({ user: 'cerro', token: 'replacement-token' });
      held.release();
      await f.ready(spec);
      assert.equal(f.settingsRequests().length, 2, 'the replacement credential must load settings');
      assert.equal(f.settingsRequests().at(-1).auth, 'Bearer replacement-token');
      assert.equal(await f.page.evaluate(() => window.ankiquestSession?.token), 'replacement-token');
      await f.close(spec);
      await f.deliver(null);
      await f.open(spec);
      await f.page.locator(spec.input).waitFor();
      assert.equal(f.settingsRequests().length, 2, 'neither native credential may survive removal in the manual cache');
    });
  }

  test(`${spec.name}: a native account change discards settings and ignores its pending save`, async t => {
    const f = await fixture(t);
    await f.open(spec);
    await f.ready(spec);
    const held = f.holdNext(200);
    if (spec.name === 'freezes') await f.page.locator('#freeze-enabled').check();
    else await f.page.locator('#nudge-enabled').check();
    await f.page.locator(spec.id).getByRole('button', { name: spec.save, exact: true }).click();
    await held.arrived;
    await f.deliver(null);
    held.release();
    await f.page.locator(spec.input).waitFor();
    await f.page.evaluate(() => new Promise(resolve => setTimeout(resolve, 50)));
    assert.equal(await f.page.locator(spec.ready).count(), 0, 'a completed old save must not restore the old account settings');
    assert.doesNotMatch(await f.page.locator(spec.id).getByRole('status').textContent(), /saved|protection is on/i);
    await f.close(spec);
    await f.open(spec);
    await f.page.locator(spec.input).waitFor();
    assert.equal(f.settingsRequests().length, 2);
  });

  test(`${spec.name}: an old save cannot overwrite edits made with replacement credentials`, async t => {
    const f = await fixture(t);
    await f.open(spec);
    await f.ready(spec);
    const held = f.holdNext(200);
    await f.page.locator(spec.id).getByRole('button', { name: spec.save, exact: true }).click();
    await held.arrived;
    f.state.acceptedToken = 'replacement-token';
    await f.deliver({ user: 'cerro', token: 'replacement-token' });
    await f.ready(spec);
    const preference = f.page.locator(spec.name === 'freezes' ? '#freeze-enabled' : '#nudge-enabled');
    await preference.check();
    const response = f.page.waitForResponse(response => response.request().method() === 'POST');
    held.release();
    await response;
    await f.page.evaluate(() => new Promise(resolve => setTimeout(resolve, 50)));
    assert.equal(await preference.isChecked(), true);
    assert.match(await f.page.locator(spec.id).getByRole('status').textContent(), /unsaved/i);
    assert.equal(f.settingsRequests().at(-1).auth, 'Bearer replacement-token');
  });

  test(`${spec.name}: invalid saved credentials are cleared and can be replaced without reopening`, async t => {
    const f = await fixture(t);
    f.state.acceptedToken = 'replacement-token';
    await f.open(spec);
    await f.page.locator(spec.id).getByRole('status').filter({ hasText: /not accepted|invalid|expired/i }).waitFor();
    assert.equal(await f.page.locator(spec.input).inputValue(), '');
    assert.equal(await f.page.locator(spec.unlock).getByRole('button').isEnabled(), true);
    assert.notEqual(await f.page.evaluate(() => window.ankiquestSession?.token), 'saved-token', 'a rejected native credential must be discarded');
    assert.equal(f.settingsRequests().length, 1);
    await f.unlock(spec);
    await f.close(spec);
    const other = dialogs.find(candidate => candidate !== spec);
    await f.open(other);
    await f.ready(other);
    assert.equal(f.settingsRequests().at(-1).auth, 'Bearer replacement-token');
    await f.noCredentialPersistence('saved-token', 'replacement-token');
  });

  test(`${spec.name}: expired credentials during save expose a retryable token form`, async t => {
    const f = await fixture(t);
    await f.open(spec);
    await f.ready(spec);
    f.state.acceptedToken = 'replacement-token';
    await f.page.locator(spec.id).getByRole('button', { name: spec.save, exact: true }).click();
    await f.page.locator(spec.input).waitFor();
    await f.page.locator(spec.id).getByRole('status').filter({ hasText: /no longer accepted/i }).waitFor();
    assert.equal(await f.page.locator(spec.input).inputValue(), '');
    await f.unlock(spec);
    assert.equal(f.settingsRequests().at(-1).auth, 'Bearer replacement-token');
  });

  test(`${spec.name}: failed loading keeps an enabled retry control and can recover`, async t => {
    const f = await fixture(t);
    f.state.nextFailure = 'network';
    await f.open(spec);
    await f.page.locator(spec.id).getByRole('status').filter({ hasText: /failed|could not|try again|network/i }).waitFor();
    const retry = f.page.locator(spec.id).getByRole('button', { name: /retry|try again|load my/i });
    assert.equal(await retry.isEnabled(), true);
    await retry.click();
    await f.ready(spec);
    assert.equal(f.settingsRequests().length, 2);
  });
}

for (const spec of [...dialogs, incoming]) {
  test(spec.name + ': current cookie account opens and saves without a token', async t => {
    const f = await fixture(t, null, 'cerro', {modern:true, cookieUser:'cerro'});
    await f.open(spec); await f.ready(spec);
    assert.equal(await f.page.locator(spec.input).count(), 0);
    await f.page.locator(spec.id).getByRole('button', {name:spec.save, exact:true}).click();
    await f.page.locator(spec.id).getByRole('status').filter({hasText:/saved|protection is off/}).waitFor();
    assert(f.settingsRequests().every(request => !request.auth));
    assert.equal(f.settingsRequests().find(request => request.method === 'POST').csrf, '1');
  });
  test(spec.name + ': changing cookie identity closes the dialog and ignores its pending response', async t => {
    const f = await fixture(t, null, 'cerro', {modern:true, cookieUser:'cerro'});
    const held=f.holdNext(200);
    await f.open(spec); await held.arrived;
    f.state.cookieUser='hill';
    await f.page.evaluate(() => AnkiQuestSite.status(true));
    await f.page.waitForFunction(id => !document.querySelector(id).open, spec.id);
    held.release();
    await f.page.evaluate(() => new Promise(resolve => setTimeout(resolve, 50)));
    assert.equal(await f.page.locator(spec.ready).count(), 0);
    assert.equal(await f.page.evaluate(() => settingsFetchSignals.at(-1).aborted), true);
  });
}
test('a manually verified account uses the server session in the next settings dialog and after reload', async t => {
  const f=await fixture(t,null,'cerro',{modern:true});
  await f.open(dialogs[0]);await f.unlock(dialogs[0]);await f.close(dialogs[0]);
  await f.open(dialogs[1]);await f.ready(dialogs[1]);await f.close(dialogs[1]);
  assert.equal(f.settingsRequests()[0].auth,'Bearer saved-token');
  assert.equal(f.settingsRequests()[1].auth,undefined);
  await f.page.reload();await f.page.locator('#manage-freezes').waitFor();
  await f.open(dialogs[0]);await f.ready(dialogs[0]);
  assert.equal(f.settingsRequests().at(-1).auth,undefined);
  await f.noCredentialPersistence('saved-token');
});

for(const replacement of [{user:'hill',token:'hill-token'},null]) {
  test('a pending manual sign-in yields to native '+(replacement?'account replacement':'account removal'), async t=>{
    const f=await fixture(t,null,'cerro',{modern:true});
    const spec=dialogs[0],held=f.holdSession();
    await f.open(spec);await f.page.locator(spec.input).fill('saved-token');
    await f.page.locator(spec.unlock).getByRole('button').click();await held.arrived;
    if(replacement)f.state.acceptedToken='hill-token';
    // Supplying and then removing a native account models account removal during the pending manual sign-in.
    if(!replacement)await f.deliver({user:'cerro',token:'saved-token'});
    await f.deliver(replacement);
    const corrected=f.page.waitForResponse(response=>new URL(response.url()).pathname===(replacement?'/auth/session':'/auth/logout') && (!replacement || response.request().headers().authorization==='Bearer hill-token'),{timeout:3000});
    held.release();await corrected;
    await f.page.evaluate(()=>AnkiQuestSite.status(true));
    assert.equal(f.state.cookieUser,replacement?.user||null);
    assert.equal(await f.page.locator(spec.ready).count(),0,'old manual settings must not reappear');
  });
}
test('disconnect waits out a pending sign-in and prevents stale cookie reconnection', async t=>{
  const f=await fixture(t,null,'cerro',{modern:true});
  const spec=dialogs[0],held=f.holdSession();
  await f.open(spec);await f.page.locator(spec.input).fill('saved-token');
  await f.page.locator(spec.unlock).getByRole('button').click();await held.arrived;
  await f.page.evaluate(()=>{window.disconnectFinished=false;window.disconnectDone=AnkiQuestSite.disconnectMember().then(()=>{disconnectFinished=true;});});
  await f.page.evaluate(()=>new Promise(resolve=>setTimeout(resolve,100)));
  assert.equal(await f.page.evaluate(()=>disconnectFinished),false,'logout must wait for the earlier cookie mutation');
  held.release();await f.page.evaluate(()=>disconnectDone);
  await f.page.evaluate(()=>new Promise(resolve=>setTimeout(resolve,100)));
  assert.equal(f.state.cookieUser,null);
  assert.equal(await f.page.locator(spec.ready).count(),0);
});

test('a successful manual account switch keeps its cookie when its identity event closes the previous dialog', async t=>{
  const f=await fixture(t,null,'cerro',{modern:true,cookieUser:'hill'});
  const spec=dialogs[0];await f.open(spec);await f.page.locator(spec.input).fill('saved-token');
  await f.page.locator(spec.unlock).getByRole('button').click();
  await f.page.waitForFunction(id=>!document.querySelector(id).open,spec.id);
  assert.equal(f.state.cookieUser,'cerro');
  await f.open(spec);await f.ready(spec);
  assert.equal(f.settingsRequests().at(-1).auth,undefined);
});

test('canceling a queued replacement cannot leave the superseded sign-in cookie installed', async t=>{
  const f=await fixture(t,null,'cerro',{modern:true});
  const spec=dialogs[0],held=f.holdSession();
  await f.open(spec);await f.page.locator(spec.input).fill('saved-token');
  await f.page.locator(spec.unlock).getByRole('button').click();await held.arrived;
  await f.close(spec);f.state.acceptedToken='hill-token';
  const verified=f.page.waitForResponse(response=>new URL(response.url()).pathname==='/api/community/reminders/hill');
  await f.page.evaluate(()=>{window.queuedActive=true;window.queuedConnection=AnkiQuestSite.connectMember('hill','hill-token',()=>queuedActive).catch(error=>error.name);});
  await verified;await f.page.evaluate(()=>new Promise(resolve=>setTimeout(resolve,50)));
  await f.page.evaluate(()=>{queuedActive=false;});held.release();
  assert.equal(await f.page.evaluate(()=>queuedConnection),'AbortError');
  assert.equal(f.state.cookieUser,null);
});

for(const spec of dialogs) {
  test(spec.name+': removing native credentials cannot reopen settings through its old cookie', async t=>{
    const f=await fixture(t,savedSession,'cerro',{modern:true,cookieUser:'cerro'});
    await f.open(spec);await f.ready(spec);await f.deliver(null);
    await f.page.locator(spec.input).waitFor();await f.close(spec);await f.open(spec);
    await f.page.evaluate(async()=>{await AnkiQuestSite.member('cerro');await new Promise(resolve=>setTimeout(resolve,200));});
    assert.equal(f.settingsRequests().length,1);
    assert.equal(await f.page.locator(spec.ready).count(),0);
  });
}

for(const spec of dialogs) {
  test(spec.name+': a first null native event clears settings already loaded with a cookie', async t=>{
    const f=await fixture(t,null,'cerro',{modern:true,cookieUser:'cerro'});
    await f.open(spec);await f.ready(spec);await f.deliver(null);
    assert.equal(await f.page.locator(spec.ready).count(),0);
    await f.page.locator(spec.input).waitFor();
    assert.equal(f.settingsRequests().filter(request=>request.method==='POST').length,0);
  });
}

test('native account cleanup closes recipient preferences while repeated valid identity preserves edits', async t=>{
  const f=await fixture(t);
  await f.page.locator('#manage-received-notifications').click();
  await f.page.locator('[data-notification-settings]').waitFor();
  await f.page.locator('#notification-enabled').uncheck();await f.deliver();
  assert.equal(await f.page.locator('#notification-preferences').evaluate(dialog=>dialog.open),true);
  assert.equal(await f.page.locator('#notification-enabled').isChecked(),false);
  await f.deliver(null);
  assert.equal(await f.page.locator('#notification-preferences').evaluate(dialog=>dialog.open),false);
  await f.page.waitForFunction(()=>document.getElementById('notification-preferences').innerHTML==='');
});

test('incoming: native credentials load and save without token reentry', async t=>{
  const f=await fixture(t);await f.open(incoming);await f.ready(incoming);
  assert.equal(await f.page.locator(incoming.input).count(),0);
  await f.page.locator('[data-unsubscribed-sender="hill"]').check();
  await f.page.locator('[data-notification-save]').click();
  await f.page.locator('#notification-status').filter({hasText:'Preferences saved'}).waitFor();
  assert.deepEqual(JSON.parse(f.settingsRequests().at(-1).body),{enabled:true,unsubscribed_senders:['hill']});
  assert(f.settingsRequests().every(request=>request.auth==='Bearer saved-token'));
  await f.noCredentialPersistence('saved-token');
});

test('incoming: subscriptions remain editable while the overall alert switch is off', async t=>{
  const f=await fixture(t);await f.open(incoming);await f.ready(incoming);
  await f.page.locator('#notification-enabled').uncheck();
  await f.page.locator('[data-unsubscribed-sender="hill"]').check();
  await f.page.locator('[data-notification-save]').click();
  await f.page.locator('#notification-status').filter({hasText:'Preferences saved'}).waitFor();
  assert.deepEqual(JSON.parse(f.settingsRequests().at(-1).body),{enabled:false,unsubscribed_senders:['hill']});
  assert.equal(await f.page.locator('[data-unsubscribed-sender="hill"]').isEnabled(),true);
});

test('incoming: unrelated configured members are not shown as current senders',async t=>{
  const f=await fixture(t,savedSession,'cerro',{sharing:[]});await f.open(incoming);await f.ready(incoming);
  assert.equal(await f.page.locator('[data-unsubscribed-sender]').count(),0);
  await f.page.getByText('No one is sharing decks with you yet.',{exact:false}).waitFor();
});

test('incoming: unsubscribed senders remain listed after sharing stops',async t=>{
  const f=await fixture(t,savedSession,'cerro',{sharing:[],unsubscribed:['hill']});await f.open(incoming);await f.ready(incoming);
  assert.equal(await f.page.locator('[data-unsubscribed-sender="hill"]').isChecked(),true);
});

test('incoming: existing mutes become unsubscribe choices and can be cleared to subscribe again', async t=>{
  const f=await fixture(t,savedSession,'cerro',{muted:['hill']});await f.open(incoming);await f.ready(incoming);
  const choice=f.page.locator('[data-unsubscribed-sender="hill"]');
  assert.equal(await choice.isChecked(),true);
  await f.page.getByText('Your previous mutes are selected above.',{exact:false}).waitFor();
  await f.page.locator('[data-notification-save]').click();
  await f.page.locator('#notification-status').filter({hasText:'Preferences saved'}).waitFor();
  assert.deepEqual(JSON.parse(f.settingsRequests().at(-1).body),{enabled:true,unsubscribed_senders:['hill']});
  await choice.uncheck();await f.page.locator('[data-notification-save]').click();
  await f.page.waitForFunction(()=>document.querySelector('[data-notification-status]').textContent.includes('Preferences saved')&&!document.querySelector('[data-notification-save]').disabled);
  assert.deepEqual(JSON.parse(f.settingsRequests().at(-1).body),{enabled:true,unsubscribed_senders:[]});
});

for(const width of [320,390,1440])test('incoming: unsubscribe drafts survive a failed save at '+width+'px',async t=>{
  const f=await fixture(t);await f.page.setViewportSize({width,height:844});await f.open(incoming);await f.ready(incoming);
  await f.page.locator('[data-unsubscribed-sender="hill"]').check();f.state.nextFailure=500;
  await f.page.locator('[data-notification-save]').click();
  await f.page.locator('#notification-status').filter({hasText:'Your changes are still here'}).waitFor();
  assert.equal(await f.page.locator('[data-unsubscribed-sender="hill"]').isChecked(),true);
  assert.equal(await f.page.locator('[data-notification-save]').isEnabled(),true);
  assert(await f.page.locator(incoming.id).evaluate(dialog=>dialog.scrollWidth<=dialog.clientWidth+1),'no horizontal overflow');
  await f.page.locator('[data-notification-save]').click();
  await f.page.locator('#notification-status').filter({hasText:'Preferences saved'}).waitFor();
});

for(const cookieUser of [null,'hill']) {
  test('incoming: '+(cookieUser?'another owner':'read-only access')+' stays manual',async t=>{
    const f=await fixture(t,null,'cerro',{modern:true,cookieUser});
    await f.open(incoming);await f.page.locator(incoming.input).waitFor();
    await f.page.evaluate(()=>AnkiQuestSite.status());
    assert.equal(f.settingsRequests().length,0);
    if(cookieUser) {
      await f.page.locator(incoming.input).fill('saved-token');
      await f.page.locator('[data-notification-load]').click();
      await f.page.waitForFunction(()=>!document.getElementById('notification-preferences').open);
      await f.open(incoming);await f.ready(incoming);
    } else await f.unlock(incoming);
    assert.equal(f.settingsRequests()[0].auth,'Bearer saved-token');
  });
}

test('incoming: explicit rejected native bearer never falls back to a valid cookie', async t=>{
  const f=await fixture(t,{user:'cerro',token:'wrong-token'},'cerro',{modern:true,cookieUser:'cerro'});
  await f.open(incoming);
  await f.page.locator('#notification-status').filter({hasText:'not accepted'}).waitFor();
  assert.equal(f.settingsRequests().length,1);assert.equal(f.settingsRequests()[0].auth,'Bearer wrong-token');
  assert.equal(await f.page.locator(incoming.ready).count(),0);
  await f.unlock(incoming);await f.ready(incoming);
});

test('incoming: a different native owner prevents reuse of the previous owner cookie', async t=>{
  const f=await fixture(t,{user:'hill',token:'hill-token'},'cerro',{modern:true,cookieUser:'cerro'});
  await f.open(incoming);await f.page.locator(incoming.input).waitFor();
  await f.page.evaluate(()=>AnkiQuestSite.status());
  assert.equal(f.settingsRequests().length,0);
});

test('incoming: revoked cookie clears preferences and allows manual reconnect', async t=>{
  const f=await fixture(t,null,'cerro',{modern:true,cookieUser:'cerro'});
  await f.open(incoming);await f.ready(incoming);f.state.nextFailure=401;
  await f.page.locator('[data-notification-save]').click();
  await f.page.locator(incoming.input).waitFor();
  assert.equal(await f.page.locator(incoming.ready).count(),0);
  await f.unlock(incoming);await f.ready(incoming);
});

test('incoming: pending native save cannot repaint a changed account',async t=>{
  const f=await fixture(t);await f.open(incoming);await f.ready(incoming);
  const held=f.holdNext(200);await f.page.locator('[data-notification-save]').click();await held.arrived;
  await f.deliver({user:'hill',token:'hill-token'});held.release();
  await f.page.waitForFunction(()=>!document.getElementById('notification-preferences').open);
  await f.page.waitForFunction(()=>document.getElementById('notification-preferences').innerHTML==='');
  assert.equal(await f.page.evaluate(()=>settingsFetchSignals.at(-1).aborted),true);
});

test('incoming: connecting once shares its owner session with deck settings and reload',async t=>{
  const f=await fixture(t,null,'cerro',{modern:true});
  await f.open(incoming);await f.unlock(incoming);await f.close(incoming);
  await f.open(dialogs[1]);await f.ready(dialogs[1]);await f.close(dialogs[1]);
  await f.page.reload();await f.page.locator(incoming.open).waitFor();
  await f.open(incoming);await f.ready(incoming);
  assert.equal(f.settingsRequests()[0].auth,'Bearer saved-token');
  assert(f.settingsRequests().slice(1).every(request=>!request.auth));
  await f.noCredentialPersistence('saved-token');
});

test('incoming: the leaderboard entry resolves the cookie owner before loading',async t=>{
  const f=await fixture(t,null,'cerro',{modern:true,cookieUser:'cerro'});
  await f.page.evaluate(()=>openNotificationPreferences());await f.ready(incoming);
  assert.equal(f.settingsRequests()[0].url,origin+'/api/deck-subscriptions/cerro');
  assert.equal(f.settingsRequests()[0].auth,undefined);
});

test('incoming: the leaderboard manual player can be corrected after a failed attempt',async t=>{
  const f=await fixture(t,null);await f.page.evaluate(()=>openNotificationPreferences());
  await f.page.locator('#notification-player').fill('hill');
  await f.page.locator(incoming.input).fill('wrong-token');await f.page.locator('[data-notification-load]').click();
  await f.page.locator('#notification-status').filter({hasText:'not accepted'}).waitFor();
  await f.page.locator('#notification-player').fill('cerro');await f.unlock(incoming);
  assert.equal(f.settingsRequests().at(-1).url,origin+'/api/deck-subscriptions/cerro');
});
