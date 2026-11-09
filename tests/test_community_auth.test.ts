// Run: node --experimental-strip-types --test tests/test_community_auth.test.ts
// Set PLAYWRIGHT_MODULE if Playwright is not available on NODE_PATH.
import assert from 'node:assert/strict';
import {test, before, after} from 'node:test';
import type {TestContext} from 'node:test';
import type {Browser, Page} from 'playwright';
import {fulfillAsset, launch, page as built} from './support/web.ts';

const html = built('community');
const origin = 'http://ankiquest.test';
interface Session { user: string; token?: string }
interface ActivityItem { id: number; sender: string; kind: string; title: string; body: string; created_at: number; read_at: number | null; challenge_id?: number; action_required?: boolean }
interface Options { locale?: string; cookieUser?: string; activityItems?: ActivityItem[]; activityBefore?: number; challengeRecipients?: boolean }
interface Recorded { url: string; auth: string | undefined; csrf?: string | undefined; method: string; body: Record<string, unknown> | null }
interface Hold { arrive: () => void; released: Promise<void> }
interface Control { reject: boolean; rejectLogout: boolean; hold: Promise<void> | null; holdSession: Hold | null; cookieUser: string | null; replies: {auth: string | undefined; body: unknown}[]; receiving: boolean; automatic: boolean; muted: Record<string, boolean>; nudged: boolean; activityItems: ActivityItem[] }
interface TestWindow { storageWrites: [string, string][]; connection: Promise<unknown>; connectionActive: boolean; queuedActive: boolean; queuedConnection: Promise<unknown>; disconnectFinished: boolean; disconnectDone: Promise<void> }
const saved: Session = { user: 'cerro', token: 'saved-token' };
const tokenOwners: Record<string, string> = { 'Bearer saved-token': 'cerro', 'Bearer hill-token': 'hill' };
let browser: Browser | undefined;
before(async () => {
  browser = await launch();
});
after(async () => browser?.close());

async function fixture(t: TestContext, session: Session | null = saved, options: Options = {}): Promise<{page: Page; requests: Recorded[]; control: Control; deliver(value?: Session | null): Promise<void>; holdSession(): {arrived: Promise<void>; release: () => void}}> {
  const context = await browser!.newContext({ viewport: { width: 390, height: 844 }, locale: options.locale || 'en-US' });
  t.after(() => context.close());
  const page = await context.newPage();
  page.setDefaultTimeout(10000);
  page.setDefaultNavigationTimeout(15000);
  const errors: string[] = [], requests: Recorded[] = [];
  const control: Control = { reject: false, rejectLogout: false, hold: null, holdSession: null, cookieUser:options.cookieUser || null, replies: [], receiving:false, automatic:false, muted:{}, nudged:false,
    activityItems:(options.activityItems || [{id:1,sender:'hill',kind:'message',title:'Saved encouragement',body:'Nice studying!',created_at:Math.floor(Date.now()/1000),read_at:null}]).map(item=>({...item})) };
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(session => {
    const test = window as unknown as TestWindow;
    test.storageWrites = [];
    const setItem = Storage.prototype.setItem;
    Storage.prototype.setItem = function(key, value) {
      test.storageWrites.push([key, value]);
      return setItem.call(this, key, value);
    };
    if (session) window.ankiquestSession = session;
  }, session);
  await page.route(`${origin}/**`, async route => {
    const request = route.request(), url = new URL(request.url());
    if (url.pathname === '/community') return route.fulfill({ contentType: 'text/html', body: html });
    if (await fulfillAsset(route, url.pathname)) return;
    if(url.pathname === '/api/avatars')return route.fulfill({contentType:'application/json',body:'{}'});
    if (url.pathname === '/auth/status') return route.fulfill({contentType: 'application/json', body: JSON.stringify({private_site:false, authenticated:true, member:control.cookieUser ? {user:control.cookieUser} : null})});
    if (url.pathname === '/auth/session') {
      const held = control.holdSession;
      if (held) { control.holdSession = null; held.arrive(); await held.released; }
      const owner = tokenOwners[request.headers().authorization ?? ''];
      if (!owner) return route.fulfill({status:401, body:''});
      control.cookieUser = owner;
      return route.fulfill({status:204});
    }
    if (url.pathname === '/auth/logout') {
      if (control.rejectLogout) { return route.fulfill({status:503}); }
      control.cookieUser = null;
      return route.fulfill({status:204});
    }
    let data: unknown;
    if (url.pathname === '/api/community') data = { meta: {}, players: [{ user: 'cerro', display: 'Cerro' }, { user: 'hill', display: 'Hill' }] };
    else if (url.pathname.startsWith('/api/friend-nudges/')) {
      requests.push({url:url.pathname,auth:request.headers().authorization,method:request.method(),body:request.postDataJSON()});
      if(control.hold)await control.hold;
      if(request.method()==='POST') {
        if(url.pathname.endsWith('/receiving'))control.receiving=request.postDataJSON().enabled;
        else if(url.pathname.endsWith('/automatic'))control.automatic=request.postDataJSON().enabled;
        else if(url.pathname.includes('/senders/'))control.muted[decodeURIComponent(url.pathname.split('/').pop()!)]=!request.postDataJSON().enabled;
        else control.nudged=true;
        return route.fulfill({status:204});
      }
      data={receiving:control.receiving,automatic_receiving:control.automatic,friends:[{user:'hill',display:'Hill',enabled:true,sent_today:control.nudged,muted_by_me:!!control.muted.hill},{user:'friend',display:'Friend',enabled:false,sent_today:false,muted_by_me:!!control.muted.friend}]};
    }
    else if (url.pathname.startsWith('/api/activity/')) {
      if(url.pathname.endsWith('/read')&&request.method()==='POST') {
        const body: {ids?: number[]; through?: number} = request.postDataJSON(),ids=new Set(body.ids||[]);
        for(const item of control.activityItems)if(ids.has(item.id)||(body.through&&item.id<=body.through))item.read_at=Date.now()/1000;
        return route.fulfill({status:204});
      }
      const all=control.activityItems.slice().sort((a,b)=>b.id-a.id),attention=all.filter(item=>item.action_required);
      let items=all.filter(item=>!url.searchParams.has('before')||item.id<Number(url.searchParams.get('before')));
      const category=url.searchParams.get('category');
      if(category==='needs_action')items=items.filter(item=>item.action_required);
      else if(category==='messages')items=items.filter(item=>['message','reply','nudge'].includes(item.kind));
      else if(category==='challenges')items=items.filter(item=>item.kind?.startsWith('challenge_'));
      else if(category==='deck_completions')items=items.filter(item=>item.kind==='completion');
      else if(category==='study_updates')items=items.filter(item=>!['message','reply','nudge','completion'].includes(item.kind)&&!item.kind?.startsWith('challenge_'));
      if(url.searchParams.get('unread_only')==='true')items=items.filter(item=>!item.read_at);
      data={items,attention:attention.slice(0,3),action_count:attention.length,latest_id:all[0]?.id||null,unread_count:all.filter(item=>!item.read_at).length,next_before:options.activityBefore||null};
    }
    else if (url.pathname.startsWith('/api/reply/')) {
      control.replies.push({auth:request.headers().authorization,body:request.postDataJSON()});
      data = {to:'hill'};
    }
    else if (url.pathname.startsWith('/api/community/')) {
      requests.push({ url: url.pathname, auth: request.headers().authorization, csrf: request.headers()['x-ankiquest-csrf'], method: request.method(), body: request.postDataJSON() });
      if (control.hold) await control.hold;
      const cookieAuthorized=!request.headers().authorization && control.cookieUser===decodeURIComponent(url.pathname.split('/').pop()!);
      if(cookieAuthorized && request.method()==='POST' && request.headers()['x-ankiquest-csrf']!=='1')return route.fulfill({status:403,body:'{}'});
      if (control.reject || (!cookieAuthorized && !['Bearer saved-token', 'Bearer hill-token'].includes(request.headers().authorization!))) return route.fulfill({ status: 401, body: '{}' });
      data = url.pathname.includes('/reminders/')
        ? { gentle_daily: false, reminder_hour: 18, quiet_start: 22, quiet_end: 9, daily_limit: 3, ...(request.method() === 'POST' ? request.postDataJSON() : {}) }
        : { challenges: [], recipients: options.challengeRecipients ? [{user:'hill',display:'Hill'},{user:'cerro',display:'Cerro'}] : [] };
    } else return route.fulfill({ status: 404, body: '' });
    await route.fulfill({ contentType: 'application/json', body: JSON.stringify(data) });
  });
  await page.goto(`${origin}/community#reminders`);
  await page.waitForFunction(() => document.querySelector('#community-app')!.getAttribute('aria-busy') === 'false');
  t.after(() => assert.deepEqual(errors, []));
  return { page, requests, control,
    holdSession() {
      let arrive!: () => void, release!: () => void;
      const arrived = new Promise<void>(resolve => { arrive = resolve; }), released = new Promise<void>(resolve => { release = resolve; });
      control.holdSession = {arrive, released};
      t.after(release);
      return {arrived, release};
    },
    async deliver(value: Session | null = saved) {
      await page.evaluate(value => { window.ankiquestSession = value; window.dispatchEvent(new CustomEvent('ankiquest-auth')); }, value);
    },
  };
}

test('Spanish community labels follow the selected language without translating player names or authored messages', async t => {
  const {page} = await fixture(t, saved, {locale:'es-ES'});
  assert.equal(await page.locator('#tab-challenges').innerText(), 'Amigos');
  assert.equal(await page.locator('#tab-reminders').innerText(), 'Recordatorios');
  await page.locator('#reminder-form').waitFor();
  assert.equal(await page.locator('#view-reminders .auth-status strong').innerText(), 'Cerro');
  await page.locator('#tab-activity').click();
  await page.getByText('Nice studying!', {exact:true}).waitFor();
});

test('Spanish friend nudges preserve the selected recipient and daily limit', async t => {
  const {page,requests}=await fixture(t,saved,{locale:'es-ES'});
  await page.getByRole('tab',{name:'Amigos',exact:true}).click();
  await page.getByRole('button',{name:'Dar un toque a amigos',exact:true}).click();
  const dialog=page.getByRole('dialog',{name:'Da un toque a tus amigos',exact:true});
  await dialog.locator('[data-nudge-user="hill"]').waitFor();
  await dialog.getByText('Permitir que mis amigos me den toques',{exact:true}).waitFor();
  await dialog.getByText('Permitir que AnkiQuest me envíe toques de progreso',{exact:true}).waitFor();
  assert.equal(await dialog.locator('[data-nudge-user="friend"]').isEnabled(),false);
  await dialog.locator('[data-nudge-user="hill"]').click();
  await dialog.getByText('¡Toque enviado!',{exact:true}).waitFor();
  await dialog.getByText('Toque enviado hoy',{exact:true}).waitFor();
  assert.equal(await dialog.locator('[data-nudge-user="hill"]').isEnabled(),false);
  assert.deepEqual(requests.filter(r=>r.method==='POST'&&r.url.startsWith('/api/friend-nudges/')).map(r=>r.body),[{recipient:'hill'}]);
});

test('Spanish achievements and thanks preserve friend messages and send contextual replies', async t => {
  const now=Math.floor(Date.now()/1000);
  const {page,control}=await fixture(t,saved,{locale:'es-ES',activityItems:[
    {id:1,sender:'hill',kind:'completion',title:'Deck complete',body:'Hill finished Friends::{0}.',created_at:now,read_at:null},
    {id:2,sender:'hill',kind:'reply',title:'💬 Hill',body:'Good job!',created_at:now,read_at:null},
  ]});
  await page.getByRole('tab',{name:'Amigos',exact:true}).click();
  const feed=page.getByRole('region',{name:'Logros de tus amigos',exact:true});
  await feed.getByText('Hill finished Friends::{0}.',{exact:true}).waitFor();
  await feed.getByRole('button',{name:'Felicitar',exact:true}).click();
  await feed.getByText('Felicitación enviada',{exact:true}).waitFor();
  await page.locator('#tab-activity').click();
  const reply=page.locator('.activity-item[data-notice="2"]');
  await reply.getByText('Good job!',{exact:true}).waitFor();
  await reply.getByRole('button',{name:'¡Gracias!',exact:true}).click();
  assert.deepEqual(control.replies.map(r=>r.body),[{notification:1,message:'¡Buen trabajo!'},{notification:2,message:'¡Gracias!'}]);
});

test('community reminders reuse the saved account, including saves, without persisting its token', async t => {
  const { page, requests } = await fixture(t);
  await page.locator('#reminder-form').waitFor();
  assert.equal(await page.locator('#auth-dialog').evaluate((dialog: HTMLDialogElement) => dialog.open), false);
  await page.locator('[name=gentle_daily]').check();
  await page.getByRole('button', { name: 'Save reminder preferences' }).click();
  await page.getByText('Your reminder preferences are saved.').waitFor();
  assert.ok(requests.every(request => request.auth === 'Bearer saved-token' && request.url.endsWith('/cerro')));
  assert.equal(requests.find(request => request.method === 'POST')!.body!.gentle_daily, true);
  const persisted = await page.evaluate(() => JSON.stringify([localStorage, sessionStorage, (window as unknown as TestWindow).storageWrites, location.href]));
  assert.ok(!persisted.includes(saved.token!));
});

test('activity suggests congratulations only for deck completions and thanks for replies', async t => {
  const now = Math.floor(Date.now() / 1000);
  const activityItems: ActivityItem[] = [
    {id:1,sender:'hill',kind:'completion',title:'Deck complete',body:'Hill finished Spanish.',created_at:now,read_at:null},
    {id:2,sender:'hill',kind:'reply',title:'💬 Hill',body:'Good job!',created_at:now,read_at:null},
    {id:3,sender:'hill',kind:'message',title:'A note from Hill',body:'Hello!',created_at:now,read_at:null},
  ];
  const {page,control} = await fixture(t,saved,{activityItems});
  await page.goto(`${origin}/community#activity`);
  const completion = page.locator('.activity-item[data-notice="1"]');
  const reply = page.locator('.activity-item[data-notice="2"]');
  const message = page.locator('.activity-item[data-notice="3"]');
  await completion.waitFor();
  assert.equal(await completion.getByRole('button',{name:'Good job!'}).count(),1);
  assert.equal(await reply.getByRole('button',{name:'Good job!'}).count(),0);
  assert.equal(await message.getByRole('button',{name:'Good job!'}).count(),0);
  await reply.getByRole('button',{name:'Thanks!'}).click();
  await page.locator('#activity-action-status').filter({hasText:'reply was sent'}).waitFor();
  assert.deepEqual(control.replies.at(-1)!.body,{notification:2,message:'Thanks!'});
});

test('activity filters keep read invitations actionable and mark all across categories', async t => {
  const now=Math.floor(Date.now()/1000);
  const {page,control}=await fixture(t,saved,{activityItems:[
    {id:1,sender:'hill',kind:'message',title:'A message',body:'Hello',created_at:now,read_at:null},
    {id:2,sender:'hill',kind:'completion',title:'Deck complete',body:'Spanish done',created_at:now,read_at:null},
    {id:3,sender:'hill',kind:'challenge_invite',title:'Study together',body:'Join me',created_at:now,read_at:now,challenge_id:42,action_required:true},
    {id:4,sender:'',kind:'reminder_daily',title:'Study reminder',body:'Keep going',created_at:now,read_at:now},
  ]});
  await page.goto(`${origin}/community#activity`);
  const inbox=page.locator('#view-activity');
  await inbox.getByRole('heading',{name:/Needs action/}).waitFor();
  for(const width of [320,390,1440]){
    await page.setViewportSize({width,height:844});
    const overflow=await page.evaluate(()=>({viewport:innerWidth,content:document.documentElement.scrollWidth}));
    assert(overflow.content<=overflow.viewport,`Activity must fit ${width}px: ${JSON.stringify(overflow)}`);
  }
  assert.equal(await inbox.locator('.activity-attention-item').count(),1,'read invitation still needs action');
  await inbox.locator('[data-activity-category=deck_completions]').click();
  await inbox.locator('.activity-item[data-notice="2"]').waitFor();
  assert.equal(await inbox.locator('.activity-item[data-notice="1"]').count(),0);
  await inbox.locator('[data-activity-unread]').click();
  await inbox.locator('.activity-item[data-notice="2"]').waitFor();
  await inbox.locator('[data-activity-category=study_updates]').click();
  await inbox.getByText('No unread notifications here.',{exact:true}).waitFor();
  await page.getByRole('tab',{name:'Friends',exact:true}).click();
  await page.getByRole('region',{name:"Friends' achievements",exact:true}).getByText('Spanish done',{exact:true}).waitFor();
  await page.getByRole('tab',{name:'Activity',exact:false}).click();
  await inbox.locator('[data-activity-unread]').click();
  await inbox.locator('.activity-item[data-notice="4"]').waitFor();
  await inbox.locator('[data-activity-category=needs_action]').click();
  await inbox.locator('.activity-item[data-notice="3"]').waitFor();
  await inbox.locator('[data-activity-category=deck_completions]').click();
  await inbox.locator('.activity-item[data-notice="2"]').waitFor();
  await inbox.locator('[data-activity-read-all]').click();
  await inbox.locator('[data-activity-read-all]').waitFor({state:'detached'});
  assert(control.activityItems.every(item=>item.read_at),'mark all includes notifications outside the selected category');
  await inbox.locator('[data-activity-category=all]').click();
  await inbox.getByRole('heading',{name:/Needs action/}).waitFor();
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
  assert.equal(await page.locator('#auth-dialog').evaluate((dialog: HTMLDialogElement) => dialog.open), false);
});

test('a rejected disconnect preserves the native account and can be retried', async t => {
  const {page, control} = await fixture(t);
  const view = page.locator('#view-reminders');
  await view.locator('#reminder-form').waitFor();
  control.rejectLogout = true;
  await view.locator('[data-disconnect]').click();
  const errors = page.locator('.auth-status [role=alert]').filter({hasText:'Could not disconnect'});
  await errors.first().waitFor({state:'attached'});
  assert.deepEqual(await page.evaluate(() => window.ankiquestSession), saved);
  await view.getByRole('alert').filter({hasText:'Could not disconnect'}).waitFor();
  await view.locator('#reminder-form').waitFor();

  control.rejectLogout = false;
  await view.locator('[data-disconnect]').click();
  await view.locator('#reminder-form').waitFor({state:'detached'});
});

test('a failed challenge save stays visible after closing the form', async t => {
  const {page} = await fixture(t, saved, {challengeRecipients:true});
  await page.route(`${origin}/api/community/challenges/cerro`, async route => {
    if (route.request().method() !== 'POST') { return route.fallback(); }
    return route.fulfill({status:503, json:{error:'Goal service unavailable'}});
  });
  await page.locator('#tab-challenges').click();
  await page.locator('[data-create-challenge]').click();
  await page.locator('#challenge-form [name=recipients]').check();
  await page.getByRole('button', {name:'Send invitation'}).click();
  await page.locator('#challenge-status').getByText('Goal service unavailable').waitFor();
  await page.getByRole('button', {name:'Close new challenge'}).click();
  assert.equal(await page.locator('#challenge-action-status').innerText(), 'Goal service unavailable');
});

test('an old challenge response cannot unlock the new account’s submission', async t => {
  const {page, deliver} = await fixture(t, saved, {challengeRecipients:true});
  const hold = () => {
    let arrive!: () => void, release!: () => void;
    const arrived = new Promise<void>(resolve => { arrive = resolve; });
    const released = new Promise<void>(resolve => { release = resolve; });
    t.after(release);
    return {arrive, arrived, release, released};
  };
  const older = hold(), newer = hold();
  await page.route(`${origin}/api/community/challenges/*`, async route => {
    if (route.request().method() !== 'POST') { return route.fallback(); }
    const pending = route.request().url().endsWith('/cerro') ? older : newer;
    pending.arrive();
    await pending.released;
    return route.fulfill({status:503, json:{error:'Goal service unavailable'}});
  });
  const open = page.locator('[data-create-challenge]');
  const submit = async () => {
    await open.click();
    await page.locator('#challenge-form [name=recipients]').check();
    await page.getByRole('button', {name:'Send invitation'}).click();
  };
  await page.locator('#tab-challenges').click();
  await submit();
  await older.arrived;
  await deliver({user:'hill',token:'hill-token'});
  await submit();
  await newer.arrived;

  const oldResponse = page.waitForResponse(response => response.url().endsWith('/challenges/cerro') && response.status() === 503);
  older.release();
  await (await oldResponse).finished();
  await page.getByRole('button', {name:'Close new challenge'}).click();
  await open.click();
  assert.equal(await page.locator('#challenge-dialog').evaluate((dialog: HTMLDialogElement) => dialog.open), false);
  newer.release();
});

test('standalone visitors can connect a different player with their own token', async t => {
  const { page, requests, control } = await fixture(t, null);
  await page.locator('#connect-button').click();
  await page.locator('#auth-user').selectOption('hill');
  await page.locator('#auth-token').fill('hill-token');
  await page.locator('#auth-submit').click();
  await page.locator('#reminder-form').waitFor();
  assert.ok(requests.every(request => request.url.endsWith('/hill') && (request.auth === 'Bearer hill-token' || !request.auth)));
  assert.equal(control.cookieUser, 'hill', 'the token is exchanged for an owner session');
});

test('an account change ignores the old response and never mixes account credentials', async t => {
  const { page, requests, control, deliver } = await fixture(t, null);
  let release!: () => void;
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
  let release!: () => void;
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
  assert.equal(requests.find(request=>request.method==='POST')!.csrf,'1');
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

test('friend nudges select the recipient and show opt-out and daily limits', async t => {
  const {page,requests}=await fixture(t);
  await page.getByRole('tab',{name:'Friends',exact:true}).click();
  await page.getByRole('button',{name:'Nudge friends',exact:true}).click();
  const dialog=page.getByRole('dialog',{name:'Nudge your friends',exact:true});
  await dialog.locator('[data-nudge-user="hill"]').waitFor();
  assert.equal(await dialog.locator('[data-nudge-user="friend"]').isEnabled(),false);
  await dialog.locator('[data-nudge-receiving]').check();
  await dialog.getByText('Preference saved.',{exact:true}).waitFor();
  await dialog.locator('[data-nudge-user="hill"]').click();
  await dialog.getByText('Nudge sent!',{exact:true}).waitFor();
  assert.equal(await dialog.locator('[data-nudge-user="hill"]').isEnabled(),false);
  const posts=requests.filter(request=>request.method==='POST'&&request.url.startsWith('/api/friend-nudges/'));
  assert.deepEqual(posts.map(request=>request.body),[{enabled:true},{recipient:'hill'}]);
  assert.ok(posts.every(request=>request.auth==='Bearer saved-token'));
  assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
});

test('friend, automatic and per-friend nudge choices save independently', async t => {
  const {page,requests}=await fixture(t);
  await page.getByRole('tab',{name:'Friends',exact:true}).click();
  await page.getByRole('button',{name:'Nudge friends',exact:true}).click();
  const dialog=page.getByRole('dialog',{name:'Nudge your friends',exact:true});
  await dialog.locator('[data-nudge-user="hill"]').waitFor();
  await dialog.locator('[data-nudge-receiving]').check();
  await dialog.getByText('Preference saved.',{exact:true}).waitFor();
  await dialog.locator('[data-nudge-automatic]').check();
  await dialog.getByText('Preference saved.',{exact:true}).waitFor();
  await dialog.locator('[data-nudge-sender="hill"]').uncheck();
  await dialog.getByText('Preference saved.',{exact:true}).waitFor();
  assert.equal(await dialog.locator('[data-nudge-receiving]').isChecked(),true);
  assert.equal(await dialog.locator('[data-nudge-automatic]').isChecked(),true);
  assert.equal(await dialog.locator('[data-nudge-sender="hill"]').isChecked(),false);
  assert.equal(await dialog.locator('[data-nudge-sender="friend"]').isChecked(),true);
  assert.deepEqual(requests.filter(r=>r.method==='POST'&&r.url.startsWith('/api/friend-nudges/')).map(r=>[r.url,r.body]),[
    ['/api/friend-nudges/cerro/receiving',{enabled:true}],
    ['/api/friend-nudges/cerro/automatic',{enabled:true}],
    ['/api/friend-nudges/cerro/senders/hill',{enabled:false}],
  ]);
  await page.setViewportSize({width:320,height:700});
  assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
});

test('friend nudge dialog discards pending private data when the account changes', async t => {
  const {page,control,deliver}=await fixture(t);
  await page.getByRole('tab',{name:'Friends',exact:true}).click();
  let release!: () => void;control.hold=new Promise(resolve=>release=resolve);
  await page.getByRole('button',{name:'Nudge friends',exact:true}).click();
  await page.getByText('Loading friends…',{exact:true}).waitFor();
  await deliver(null);release();control.hold=null;
  await page.waitForTimeout(100);
  assert.equal(await page.getByRole('dialog',{name:'Nudge your friends',exact:true}).count(),0);
  assert.equal(await page.locator('[data-nudge-user]').count(),0);
});

test('friends achievements show only shared friend completions and congratulate once', async t => {
  const now=Math.floor(Date.now()/1000);
  const {page,control}=await fixture(t,saved,{activityItems:[
    {id:1,sender:'hill',kind:'completion',title:'Deck complete',body:'Hill finished Spanish.',created_at:now,read_at:null},
    {id:2,sender:'hill',kind:'reply',title:'Encouragement',body:'Good job!',created_at:now,read_at:null},
    {id:3,sender:'cerro',kind:'completion',title:'Deck complete',body:'Cerro finished Geography.',created_at:now,read_at:null},
  ]});
  await page.getByRole('tab',{name:'Friends',exact:true}).click();
  const feed=page.getByRole('region',{name:"Friends' achievements",exact:true});
  await feed.getByText('Hill finished Spanish.',{exact:true}).waitFor();
  assert.equal(await feed.locator('.friend-achievement').count(),1);
  assert.equal(await feed.getByText('Cerro finished Geography.',{exact:true}).count(),0);
  await feed.getByRole('button',{name:'Congratulate',exact:true}).click();
  await feed.getByText('Congratulations sent',{exact:true}).waitFor();
  assert.deepEqual(control.replies,[{auth:'Bearer saved-token',body:{notification:1,message:'Good job!'}}]);
  assert.equal(await feed.getByRole('button',{name:'Congratulate',exact:true}).count(),0);
  await page.getByRole('tab',{name:'Activity',exact:false}).click();
  assert.equal(await page.locator('.activity-item[data-notice="1"]').getByText('Reply sent',{exact:true}).count(),1);
});

test('friends achievements show an honest empty state and clear on account removal', async t => {
  const {page,deliver}=await fixture(t);
  await page.getByRole('tab',{name:'Friends',exact:true}).click();
  const feed=page.getByRole('region',{name:"Friends' achievements",exact:true});
  await feed.getByText('Your next shared celebration is ahead.',{exact:true}).waitFor();
  assert.equal(await feed.getByRole('button',{name:'Congratulate',exact:true}).count(),0);
  await deliver(null);
  assert.equal(await page.locator('.friend-achievement').count(),0);
  assert.equal(await page.getByRole('region',{name:"Friends' achievements",exact:true}).count(),0);
});

test('a native member without a token keeps the session cookie the app installed', async t=>{
  const {page,requests,deliver}=await fixture(t,null,{cookieUser:'cerro'});
  await page.locator('#reminder-form').waitFor();
  await deliver({user:'cerro'});
  await deliver({user:'cerro'});
  await page.locator('#reminder-form').waitFor();
  assert.equal(await page.locator('#view-reminders .auth-status strong').innerText(),'Cerro');
  await page.locator('[name=gentle_daily]').check();
  await page.getByRole('button',{name:'Save reminder preferences'}).click();
  await page.getByText('Your reminder preferences are saved.').waitFor();
  assert(requests.every(request=>!request.auth),'a cookie session sends no token');
  assert.equal(requests.find(request=>request.method==='POST')!.csrf,'1');
  await page.locator('#tab-activity').click();
  await page.getByText('Saved encouragement').waitFor();
  await deliver(null);
  await page.locator('.activity-item').waitFor({state:'detached'});
  assert.equal(await page.locator('#reminder-form').count(),0);
});

test('a native member without a token connects through the cookie when it arrives after loading', async t=>{
  const {page,requests,control,deliver}=await fixture(t,null);
  control.cookieUser='cerro';
  await deliver({user:'cerro'});
  await page.locator('#reminder-form').waitFor();
  assert(requests.length>0&&requests.every(request=>!request.auth&&request.url.endsWith('/cerro')));
});

// AnkiQuestSite.connectMember and disconnectMember share one cookie, so each waits for the
// other and an abandoned sign-in never leaves its account installed.
async function startConnection(page: Page, user: string, token: string) {
  await page.evaluate(({user, token}) => {
    const w = window as unknown as TestWindow;
    w.connectionActive = true;
    w.connection = AnkiQuestSite.connectMember(user, token, () => w.connectionActive).catch((error: Error) => error.name);
  }, {user, token});
}

for (const replacement of [{user:'hill', token:'hill-token'}, null]) {
  test('a pending manual sign-in yields to native ' + (replacement ? 'account replacement' : 'account removal'), async t => {
    const {page, control, deliver, holdSession} = await fixture(t, null);
    const held = holdSession();
    await startConnection(page, 'cerro', 'saved-token');
    await held.arrived;
    // Supplying and then removing a native account models account removal during the pending manual sign-in.
    if (!replacement) await deliver(saved);
    await deliver(replacement);
    // The caller gives up on the sign-in it started once the app's account changes.
    await page.evaluate(() => { (window as unknown as TestWindow).connectionActive = false; });
    const corrected = page.waitForResponse(response => new URL(response.url()).pathname === (replacement ? '/auth/session' : '/auth/logout') && (!replacement || response.request().headers().authorization === 'Bearer hill-token'), {timeout:3000});
    held.release();
    await corrected;
    await page.evaluate(() => AnkiQuestSite.status(true));
    assert.equal(control.cookieUser, replacement?.user ?? null);
  });
}

test('disconnect waits out a pending sign-in and prevents stale cookie reconnection', async t => {
  const {page, control, holdSession} = await fixture(t, null);
  const held = holdSession();
  await startConnection(page, 'cerro', 'saved-token');
  await held.arrived;
  await page.evaluate(() => { const w = window as unknown as TestWindow; w.disconnectFinished = false; w.disconnectDone = AnkiQuestSite.disconnectMember().then(() => { w.disconnectFinished = true; }); });
  await page.evaluate(() => new Promise(resolve => setTimeout(resolve, 100)));
  assert.equal(await page.evaluate(() => (window as unknown as TestWindow).disconnectFinished), false, 'logout must wait for the earlier cookie mutation');
  held.release();
  await page.evaluate(() => (window as unknown as TestWindow).disconnectDone);
  await page.evaluate(() => new Promise(resolve => setTimeout(resolve, 100)));
  assert.equal(control.cookieUser, null);
  assert.equal(await page.locator('#reminder-form').count(), 0);
});

test('canceling a queued replacement cannot leave the superseded sign-in cookie installed', async t => {
  const {page, control, holdSession} = await fixture(t, null);
  const held = holdSession();
  await startConnection(page, 'cerro', 'saved-token');
  await held.arrived;
  await page.evaluate(() => { (window as unknown as TestWindow).connectionActive = false; });
  const verified = page.waitForResponse(response => new URL(response.url()).pathname === '/api/community/reminders/hill');
  await page.evaluate(() => { const w = window as unknown as TestWindow; w.queuedActive = true; w.queuedConnection = AnkiQuestSite.connectMember('hill', 'hill-token', () => w.queuedActive).catch((error: Error) => error.name); });
  await verified;
  await page.evaluate(() => new Promise(resolve => setTimeout(resolve, 50)));
  await page.evaluate(() => { (window as unknown as TestWindow).queuedActive = false; });
  held.release();
  assert.equal(await page.evaluate(() => (window as unknown as TestWindow).queuedConnection), 'AbortError');
  assert.equal(await page.evaluate(() => (window as unknown as TestWindow).connection), 'AbortError');
  assert.equal(control.cookieUser, null);
});
