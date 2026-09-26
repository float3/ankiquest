// Browser integration checks against an actual AnkiQuest binary and isolated fake data.
// ANKIQUEST_BIN=/path/to/ankiquest PLAYWRIGHT_MODULE=/path/to/playwright node tests/mobile_web.cjs
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const assert = require('node:assert/strict');
const {spawn} = require('node:child_process');
const {DatabaseSync} = require('node:sqlite');
const {chromium} = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const repo = path.resolve(__dirname, '..');
const out = process.env.QA_DIR || fs.mkdtempSync(path.join(os.tmpdir(), 'ankiquest-mobile-web-'));
fs.mkdirSync(out, {recursive: true});
const run = fs.mkdtempSync(path.join(out, 'fixture-'));
const token = 'test-alice-member-token', bobToken = 'test-bob-member-token', password = 'test-read-only-password';
const checks = [], errors = [];
checks.push = function(...items) { Array.prototype.push.apply(this,items);console.log(items.join('\n')); };
let browser, server, base, log;
const exe = process.env.ANKIQUEST_BIN || path.join(repo, 'target', 'debug', process.platform === 'win32' ? 'ankiquest.exe' : 'ankiquest');
const sourceAssets = process.argv.includes('--source-assets');
async function freePort() { return new Promise((resolve, reject) => { const s = require('node:net').createServer();s.once('error', reject);s.listen(0, '127.0.0.1', () => { const port = s.address().port;s.close(() => resolve(port)); }); }); }
async function api(route, body, credential = token, expected = 200) {
  const response = await fetch(base + route, {method:body === undefined ? 'GET' : 'POST', headers:{Authorization:'Bearer ' + credential, ...(body === undefined ? {} : {'Content-Type':'application/json'})}, ...(body === undefined ? {} : {body:JSON.stringify(body)})});
  assert.equal(response.status, expected, route + ': ' + (response.status === expected ? '' : await response.text()));
  return expected === 204 ? null : response.json();
}
async function ready() { for(let n=0;n<100;n++){ if(server.exitCode !== null) throw Error('fixture exited: ' + fs.readFileSync(path.join(run,'server.log'),'utf8'));try{if((await fetch(base+'/auth/status')).ok)return;}catch{}await new Promise(resolve=>setTimeout(resolve,100)); }throw Error('fixture failed to start'); }
async function overflow(page, label) {
  const width = await page.evaluate(() => ({viewport:innerWidth, content:document.documentElement.scrollWidth}));
  assert(width.content <= width.viewport, label + ': ' + JSON.stringify(width));
}
async function signIn(page, credential) {
  await page.goto(base + '/login?next=%2Fcommunity%23challenges');
  await page.locator('#password').fill(credential);
  await page.locator('button[type=submit]').click();
  await page.waitForURL('**/community#challenges');
}
async function screenshot(page, name) { await page.screenshot({path:path.join(out,name+'.png'),fullPage:true}); }
async function main() {
  const port = await freePort();base='http://127.0.0.1:'+port;
  fs.writeFileSync(path.join(run,'password'),password);
  const db = new DatabaseSync(path.join(run,'ankiquest.db'));
  db.exec('CREATE TABLE reviews(user TEXT NOT NULL,id INTEGER NOT NULL,cid INTEGER NOT NULL,last_ivl INTEGER NOT NULL,time_ms INTEGER NOT NULL,kind INTEGER NOT NULL,PRIMARY KEY(user,id)) WITHOUT ROWID;CREATE TABLE clocks(user TEXT PRIMARY KEY,offset_west_min INTEGER NOT NULL,rollover_hour INTEGER NOT NULL);BEGIN');
  const insert=db.prepare('INSERT INTO reviews VALUES(?,?,?,?,?,?)');
  const midnight=new Date().setUTCHours(0,0,0,0);
  for(const [index,user] of ['alice','bob','cleo'].entries()) {
    db.prepare('INSERT INTO clocks VALUES(?,0,4)').run(user);
    for(let ago=16;ago>=1;ago--)for(let n=0;n<20+index*7;n++)insert.run(user,midnight-ago*86400000+12*3600000+n*5000,ago*1000+n,1,5000,1);
  }
  db.exec('COMMIT');db.close();
  fs.writeFileSync(path.join(run,'config.json'),JSON.stringify({addr:'127.0.0.1:'+port,state_dir:run,private_site:true,site_password_file:path.join(run,'password'),week_timezone:'UTC',week_rollover_hour:4,users:{alice:{display:'Alice <team>',token},bob:{display:'Bob Rivera',token:bobToken},cleo:{display:'Cléo Martín',token:'test-cleo-token'}}}));
  log=fs.openSync(path.join(run,'server.log'),'a');
  server=spawn(exe,[path.join(run,'config.json')],{windowsHide:true,stdio:['ignore',log,log]});
  await ready();
  const history=new DatabaseSync(path.join(run,'ankiquest.db'));
  history.prepare('INSERT INTO notifications(recipient,sender,title,body,day,created_at,kind) VALUES(?,?,?,?,?,?,?)').run('alice','','An earlier study update','Your activity stays available after a notification is dismissed.',Math.floor(Date.now()/86400000)-10,Date.now()-10*86400000,'message');
  history.close();
  const invited=await api('/api/community/challenges/bob',{title:'Three days with Bob',kind:'study_days',cooperative:false,target:3,duration_days:7,recipients:['alice'],request_id:'fixture-invite'},bobToken);
  const invitation=invited.challenges.find(item=>item.title==='Three days with Bob');
  await api('/api/community/challenges/alice',{title:'Our shared review goal',kind:'reviews',cooperative:true,target:50,duration_days:7,recipients:['cleo'],request_id:'fixture-active'});
  const records=new DatabaseSync(path.join(run,'ankiquest.db'));
  records.prepare('INSERT INTO notifications(recipient,sender,title,body,day,created_at,kind) VALUES(?,?,?,?,?,?,?)').run('alice','bob','A note from Bob','Nice work on your studying!',Math.floor(Date.now()/86400000),Date.now(),'message');
  records.prepare('INSERT INTO notifications(recipient,sender,title,body,day,created_at,kind) VALUES(?,?,?,?,?,?,?)').run('alice','bob','Deck complete','Bob Rivera finished Spanish for today.',Math.floor(Date.now()/86400000),Date.now(),'completion');
  records.close();
  browser=await chromium.launch({headless:true,...(process.env.PLAYWRIGHT_CHANNEL?{channel:process.env.PLAYWRIGHT_CHANNEL}:{})});
  const context=await browser.newContext({viewport:{width:390,height:844}});
  if(sourceAssets)await context.route('**/*',async route=>{
    const url=new URL(route.request().url());
    const file=url.pathname==='/community'?'community.html':['/','/week','/records','/day','/month','/all'].includes(url.pathname)?'index.html':url.pathname==='/site.js'?'site.js':url.pathname==='/site.css'?'site.css':url.pathname==='/login'?'login.html':null;
    if(file)await route.fulfill({status:200,contentType:file.endsWith('.js')?'application/javascript':file.endsWith('.css')?'text/css':'text/html',body:fs.readFileSync(path.join(repo,'static',file),'utf8')});else await route.continue();
  });
  const page=await context.newPage();page.on('pageerror',error=>errors.push(error.message));
  await signIn(page,password);
  await page.locator('#view-challenges [data-connect]').waitFor();
  assert.equal(await page.locator('[data-challenge-action]').count(),0,'shared password has no owner actions');
  await page.locator('#view-challenges [data-connect]').click();
  await page.locator('#auth-user').selectOption('alice');await page.locator('#auth-token').fill(token);await page.locator('#auth-submit').click();
  await page.locator('[data-challenge-action=accept]').waitFor();
  assert.equal((await (await page.request.get(base+'/auth/status')).json()).member.user,'alice');
  const groups=await page.locator('.challenge-group h2').allTextContents();assert.deepEqual(groups,['Invitations','In progress',"Friends' achievements"]);
  const achievements=page.getByRole('region',{name:"Friends' achievements",exact:true});
  await achievements.getByText('Bob Rivera finished Spanish for today.',{exact:true}).waitFor();
  assert.equal(await achievements.locator('.friend-achievement').count(),1);
  await achievements.getByRole('button',{name:'Congratulate',exact:true}).click();
  await achievements.getByText('Congratulations sent',{exact:true}).waitFor();
  assert((await api('/api/activity/bob',undefined,bobToken)).items.some(item=>item.body.includes('Good job!')));
  checks.push('private shared deck achievements and a real congratulations reply');
  assert.equal(await page.locator('#challenge-form').count(),0,'creator is absent until requested');
  await screenshot(page,'invitation-390-light');
  checks.push('read-only password, owner connection, invitations before active goals, creation behind explicit action');
  await page.goto(base+'/community#challenge-'+invitation.id);
  await page.locator('[data-challenge-action=accept]').waitFor();
  assert.equal(await page.locator('#auth-token:visible').count(),0,'member session survives navigation');
  await page.locator('[data-challenge-action=accept]').click();
  await page.locator('#challenge-action-status').filter({hasText:'Studying from now on'}).waitFor();
  const updated=await api('/api/community/challenges/alice');assert.equal(updated.challenges.find(item=>item.id===invitation.id).members.find(item=>item.user==='alice').progress,0);
  checks.push('stable goal URL, cookie owner continuity, acceptance without retroactive progress');
  await page.locator('#tab-challenges').click();await page.locator('[data-create-challenge]').click();
  const form=page.locator('#challenge-form');assert.equal(await form.locator('[name=target]').inputValue(),'3');assert.equal(await form.locator('[name=duration_days]').inputValue(),'7');
  await form.locator('[name=recipients][value=bob]').check();await form.getByRole('button',{name:'Send invitation'}).click();
  await page.locator('#challenge-action-status').filter({hasText:'Invitation sent'}).waitFor();
  assert.match(page.url(),/#challenge-\d+$/);
  checks.push('three-day/next-seven-day preset creates a real goal and opens its detail');
  await page.locator('#tab-activity').click();await page.locator('.activity-item').first().waitFor();
  const note=page.locator('.activity-item').filter({hasText:'A note from Bob'});await note.locator('input[name=message]').fill('Thanks, see you tomorrow!');await note.getByRole('button',{name:'Send reply',exact:true}).click();
  await page.locator('#activity-action-status').filter({hasText:'reply was sent'}).waitFor();
  assert((await api('/api/activity/bob',undefined,bobToken)).items.some(item=>item.body.includes('Thanks, see you tomorrow!')));
  const readId=await page.locator('[data-mark-read]').first().getAttribute('data-mark-read');
  await page.locator('[data-mark-read="'+readId+'"]').click();await page.locator('[data-mark-read="'+readId+'"]').waitFor({state:'detached'});
  const unreadBefore=(await api('/api/activity/alice')).unread_count;
  await page.reload();await page.locator('.activity-item').first().waitFor();
  assert.equal((await api('/api/activity/alice')).unread_count,unreadBefore);
  checks.push('real reply, persistent read state, activity survives reload');
  await page.goto(base+'/#alice');await page.locator('#manage-freezes').waitFor();
  await page.locator('#manage-freezes').click();await page.locator('#freeze-preferences').waitFor();
  assert.equal(await page.locator('#freeze-token:visible').count(),0);await page.locator('#freeze-enabled').check();await page.locator('#freeze-preferences button[type=submit]').click();await page.locator('#freeze-status').filter({hasText:'is on'}).waitFor();await page.locator('#freeze-close').click();
  await page.locator('#manage-decks').click();await page.locator('#deck-settings').waitFor();assert.equal(await page.locator('#deck-token:visible').count(),0);await page.locator('#deck-close').click();
  checks.push('freeze/deck preferences reuse owner session without repeated token');
  for(const width of [320,390,1440])for(const theme of ['light','dark']) {
    await page.setViewportSize({width,height:width<400?844:1000});await page.emulateMedia({colorScheme:theme});
    for(const [route,name] of [['/community#challenges','friends'],['/community#activity','activity'],['/community#challenge-'+invitation.id,'goal'],['/community#reminders','reminders'],['/community#overview','history'],['/?embed=1#alice','profile']]) {
      await page.goto(base+route);
      const readySelector={profile:'#manage-freezes',history:'#view-overview .kpi',friends:'#view-challenges .challenge',activity:'#view-activity .activity-item',goal:'#view-challenges [data-goal]',reminders:'#reminder-form'}[name];
      await page.locator(readySelector).first().waitFor();
      await overflow(page,name+' '+width+' '+theme);
      if(width<400&&['friends','activity','goal'].includes(name))await screenshot(page,name+'-'+width+'-'+theme);
    }
    await page.goto(base+'/community#challenges');await page.locator('[data-create-challenge]').waitFor();await page.locator('[data-create-challenge]').click();await overflow(page,'create '+width+' '+theme);await page.locator('#challenge-customize summary').click();await overflow(page,'customize '+width+' '+theme);await page.locator('[data-close=challenge-dialog]').first().click();
  }
  checks.push('320/390/1440 light/dark, focused routes and creation/customization without page overflow');
  // An acknowledgement error must not turn a notification into a dead end.
  await page.goto(base+'/community#activity');await page.locator('[data-activity-challenge]').first().waitFor();
  await page.route('**/api/activity/alice/read',route=>route.fulfill({status:503,body:'Temporarily unavailable'}));
  const goalId=await page.locator('[data-activity-challenge]').first().getAttribute('data-activity-challenge');
  await page.locator('[data-activity-challenge]').first().click();await page.locator('#view-challenges [data-goal="'+goalId+'"]').waitFor();
  await page.locator('#challenge-action-status').filter({hasText:'Read status could not be saved'}).waitFor();
  await page.unroute('**/api/activity/alice/read');
  checks.push('read acknowledgement failure does not prevent opening a goal');
  // Revoked owner reads clear personal UI, while transient connectivity retains it.
  await page.goto(base+'/community#activity');await page.locator('.activity-item').first().waitFor();
  for(const code of [401,403]) {
    await page.route('**/api/activity/alice?*',route=>route.fulfill({status:code,body:'Forbidden'}));
    await page.locator('#view-activity [data-activity-refresh]').click();await page.locator('#view-activity [data-connect]').waitFor();
    assert.equal(await page.locator('.activity-item').count(),0);assert.equal(await page.locator('[data-goal]').count(),0);
    await page.unroute('**/api/activity/alice?*');
    await page.reload();await page.locator('.activity-item').first().waitFor();
  }
  const mismatch=await page.evaluate(async({token})=>{try{await AnkiQuestSite.connectMember('bob',token);return 'accepted';}catch(error){return error.message;}},{token});
  assert.match(mismatch,/selected player/);assert.equal((await (await page.request.get(base+'/auth/status')).json()).member.user,'alice');
  checks.push('owner401/403 clears cached personal data; mismatched token cannot switch browser owner');
  await page.goto(base+'/community#activity');await page.locator('.activity-item').first().waitFor();
  await context.setOffline(true);await page.locator('#view-activity [data-activity-refresh]').click();await page.locator('#view-activity [role=alert]').waitFor();assert((await page.locator('.activity-item').count())>0,'failed refresh preserves existing activity');await context.setOffline(false);
  const saved=await page.evaluate(()=>({local:JSON.stringify(localStorage),session:JSON.stringify(sessionStorage),cookies:document.cookie}));for(const v of Object.values(saved))assert(!v.includes(token)&&!v.includes(password)&&!v.includes('ankiquest_session'));
  await page.getByRole('button',{name:'Lock site'}).click();await page.waitForURL('**/login?*');assert.equal((await page.request.get(base+'/api/activity/alice')).status(),401);
  checks.push('offline retry preserves activity; no secrets in JS storage; logout revokes owner access');
  assert.deepEqual(errors,[]);
  const result={checks,errors,sourceAssets,output:out};fs.writeFileSync(path.join(out,'results.json'),JSON.stringify(result,null,2));console.log(JSON.stringify(result,null,2));
}
main().catch(error=>{console.error(error);process.exitCode=1;}).finally(async()=>{if(browser)await browser.close();if(server)server.kill();if(log!==undefined)fs.closeSync(log);});
