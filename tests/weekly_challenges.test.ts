import assert from 'node:assert/strict';
import {test,before,after} from 'node:test';
import type {TestContext} from 'node:test';
import type {Browser} from 'playwright';
import {chromium,fulfillAsset,page as built} from './support/web.ts';
const origin='http://ankiquest.test';
interface Member {user:string;display:string;status:string;progress:number}
interface Goal {id:number;title:string;weekly:boolean;start_when_ready?:boolean;started:boolean;kind:string;cooperative:boolean;creator:string;start_at:number;end_at:number;target:number;status:string;progress:number;members:Member[]}
interface Request {path:string;auth:string|undefined;body:{action?:string;start_when_ready?:boolean}}
let browser: Browser;
before(async()=>{browser=await chromium.launch({headless:true,...(process.env.PLAYWRIGHT_CHANNEL?{channel:process.env.PLAYWRIGHT_CHANNEL}:process.platform==='win32'?{channel:'msedge'}:{})});});
after(async()=>browser?.close());
async function fixture(t: TestContext,{language='en-US',legacy=false}={}) {
  const context=await browser.newContext({viewport:{width:390,height:844},locale:language});t.after(()=>context.close());
  const page=await context.newPage();page.setDefaultTimeout(10000);
  const errors: string[]=[],requests: Request[]=[];page.on('pageerror',e=>errors.push(e.message));t.after(()=>assert.deepEqual(errors,[]));
  const now=Date.now(),suggestion={week_start:now-1000,expires_at:now+86400000,friend:{user:'hill',display:'Hill::{0}'},target:3,duration_days:7,dismissed:false,challenge_id:null as number|null};
  let challenges: Goal[]=[];
  const control: {hold:Promise<void>|null;fail:boolean}={hold:null,fail:false};
  const payload=()=>({challenges,recipients:[suggestion.friend],...(legacy?{}:{weekly_suggestion:suggestion})});
  await page.addInitScript(()=>{window.ankiquestSession={user:'cerro',token:'saved-token'};});
  await page.route(origin+'/**',async route=>{
    const req=route.request(),url=new URL(req.url()),p=url.pathname;
    const json=(data: unknown)=>route.fulfill({contentType:'application/json',body:JSON.stringify(data)});
    if(p==='/community')return route.fulfill({contentType:'text/html',body:built('community')});
    if(await fulfillAsset(route,p))return;
    if(p==='/auth/status')return json({private_site:false,authenticated:true,member:null});
    if(p==='/api/avatars')return json({});
    if(p==='/api/community')return json({meta:{},players:[{user:'cerro',display:'Cerro'},{user:'hill',display:'Hill::{0}'}]});
    if(p.startsWith('/api/language/'))return route.fulfill({status:204});
    if(p.startsWith('/api/activity/')||p.startsWith('/api/sent/'))return json({items:[],unread_count:0,next_before:null});
    if(p.startsWith('/api/community/reminders/'))return json({gentle_daily:false,reminder_hour:18,quiet_start:22,quiet_end:9,daily_limit:3});
    if(p.startsWith('/api/community/challenges/')) {
      if(req.method()==='GET')return json(payload());
      requests.push({path:p,auth:req.headers().authorization,body:req.postDataJSON()});
      if(control.hold)await control.hold;
      if(control.fail)return route.fulfill({status:400,contentType:'application/json',body:JSON.stringify({error:'Your weekly suggestion changed. Refresh and try again.'})});
      const action=(req.postDataJSON() as {action?:string}).action;
      if(action==='dismiss')suggestion.dismissed=true;
      if(action==='invite') {
        suggestion.challenge_id=1;
        challenges=[{id:1,title:'A week of steady studying',weekly:true,started:false,kind:'study_days',cooperative:false,creator:'cerro',start_at:now,end_at:suggestion.expires_at,target:3,status:'waiting',progress:0,members:[{user:'cerro',display:'Cerro',status:'accepted',progress:0},{user:'hill',display:'Hill::{0}',status:'invited',progress:0}]}];
      }
      if(action==='accept')for(const goal of challenges){goal.started=true;goal.status='active';goal.start_at=Date.now();goal.end_at=goal.start_at+7*86400000;goal.members.forEach(member=>member.status='accepted');}
      return json(payload());
    }
    return route.fulfill({status:404});
  });
  await page.goto(origin+'/community#challenges');await page.locator('[data-create-challenge]').waitFor();
  return {page,requests,control,suggestion,setGoals:(goals: Goal[])=>challenges=goals};
}
test('a suggestion sends nothing until invited; waiting has no progress timer; custom goals remain',async t=>{
  const {page,requests,suggestion}=await fixture(t);
  await page.getByRole('heading',{name:'Your weekly suggestion'}).waitFor();
  assert.equal(requests.length,0);
  await page.locator('[data-weekly-action="invite"]').click();
  await page.getByText('Waiting for your friend',{exact:true}).waitFor();
  assert.deepEqual(requests,[{path:'/api/community/challenges/cerro/weekly',auth:'Bearer saved-token',body:{week_start:suggestion.week_start,action:'invite'}}]);
  assert.equal(await page.locator('.challenge [role="progressbar"]').count(),0);
  assert.equal(await page.locator('[data-weekly-action]').count(),0);
  await page.reload();await page.getByText('Waiting for your friend',{exact:true}).waitFor();
  assert.equal(requests.length,1);
  await page.locator('[data-create-challenge]').click();
  await page.locator('#challenge-dialog[open]').waitFor();
  assert.equal(await page.locator('#challenge-form [name="recipients"]').count(),1);
});
test('custom goals offer a start-after-acceptance choice and hide progress while waiting',async t=>{
  const {page,requests,setGoals}=await fixture(t);
  await page.locator('[data-create-challenge]').click();
  const form=page.locator('#challenge-form');
  assert.equal(await form.locator('[name="start_when_ready"]').isChecked(),false);
  await form.locator('[name="recipients"]').check();
  await form.locator('[name="start_when_ready"]').check();
  await form.getByRole('button',{name:'Send invitation'}).click();
  await page.waitForFunction(()=>!document.querySelector('#challenge-dialog[open]'));
  assert.equal(requests.length,1);
  assert.equal(requests[0]!.body.start_when_ready,true);
  setGoals([{id:3,title:'Together',weekly:false,start_when_ready:true,started:false,
    kind:'reviews',cooperative:true,creator:'cerro',start_at:Date.now(),end_at:9223372036854775807,
    target:3,status:'waiting',progress:0,members:[
      {user:'cerro',display:'Cerro',status:'accepted',progress:0},
      {user:'hill',display:'Hill',status:'invited',progress:0}]}]);
  await page.reload();
  await page.getByText('Waiting for everyone',{exact:true}).waitFor();
  assert.match(await page.locator('.challenge-meta').innerText(),/Starts when everyone accepts/);
  assert.equal(await page.locator('.challenge [role="progressbar"]').count(),0);
});
test('skipping persists and keeps custom goals available without inviting anyone',async t=>{
  const {page,requests}=await fixture(t);
  await page.locator('[data-weekly-action="dismiss"]').click();
  await page.locator('.weekly-suggestion').getByText('Skipped this week. You can still start a custom friend goal.').waitFor();
  await page.reload();await page.locator('.weekly-suggestion').getByText('Skipped this week. You can still start a custom friend goal.').waitFor();
  assert.deepEqual(requests.map(r=>r.body.action),['dismiss']);
  assert.equal(await page.locator('[data-create-challenge]').isEnabled(),true);
});
test('weekly labels are Spanish and a friend name containing placeholders is preserved',async t=>{
  const {page}=await fixture(t,{language:'es-ES'});
  await page.getByRole('heading',{name:'Tu propuesta semanal'}).waitFor();
  await page.locator('.weekly-suggestion').getByText('Hill::{0}',{exact:true}).waitFor();
  await page.getByText('Los siete días empiezan cuando ambos aceptéis.').waitFor();
  await page.locator('[data-weekly-action="invite"]').click();
  await page.getByText('Esperando a tu amigo',{exact:true}).waitFor();
  await page.getByRole('heading',{name:'Una semana de estudio constante'}).waitFor();
  assert.match(await page.locator('.challenge-meta').innerText(),/Responde antes del/);
});
test('the custom goal waiting choice and state are Spanish',async t=>{
  const {page,setGoals}=await fixture(t,{language:'es-ES'});
  await page.locator('[data-create-challenge]').click();
  await page.getByText('Empezar cuando todos acepten',{exact:true}).waitFor();
  setGoals([{id:3,title:'Together',weekly:false,start_when_ready:true,started:false,
    kind:'reviews',cooperative:true,creator:'cerro',start_at:Date.now(),end_at:9223372036854775807,
    target:3,status:'waiting',progress:0,members:[
      {user:'cerro',display:'Cerro',status:'accepted',progress:0},
      {user:'hill',display:'Hill',status:'invited',progress:0}]}]);
  await page.reload();
  await page.getByText('Esperando a todos',{exact:true}).waitFor();
  assert.match(await page.locator('.challenge-meta').innerText(),/Empieza cuando todos acepten/);
});
test('the custom goal choice follows French, German and Portuguese Anki locales',async t=>{
  for(const [language,label] of [
    ['fr-FR','Commencer quand tout le monde accepte'],
    ['de-DE','Starten, wenn alle zugesagt haben'],
    ['pt-PT','Começar quando todos aceitarem'],
  ] as const){
    const {page}=await fixture(t,{language});
    await page.locator('[data-create-challenge]').click();
    await page.getByText(label,{exact:true}).waitFor();
    assert.equal(await page.locator('html').getAttribute('lang'),language.slice(0,2));
    await page.locator('.weekly-suggestion').getByText('Hill::{0}',{exact:true}).waitFor();
  }
});
test('double clicks do not duplicate requests and failed choices can be retried',async t=>{
  const {page,requests,control}=await fixture(t);
  let release!: ()=>void;control.hold=new Promise<void>(r=>release=r);control.fail=true;
  await page.locator('[data-weekly-action="invite"]').evaluate((button: HTMLButtonElement)=>{button.click();button.click();});
  await page.waitForFunction(()=>document.querySelector<HTMLButtonElement>('[data-weekly-action="invite"]')!.disabled);
  release();
  await page.getByText('Your weekly suggestion changed. Refresh and try again.').waitFor();
  assert.equal(requests.length,1);assert.equal(await page.locator('[data-weekly-action="invite"]').isEnabled(),true);
  control.fail=false;control.hold=null;
  await page.locator('[data-weekly-action="invite"]').click();await page.getByText('Waiting for your friend',{exact:true}).waitFor();
  assert.equal(requests.length,2);
});
test('older servers still show the custom goal creator',async t=>{
  const {page}=await fixture(t,{legacy:true});
  assert.equal(await page.locator('.weekly-suggestion').count(),0);
  await page.locator('[data-create-challenge]').click();await page.locator('#challenge-dialog[open]').waitFor();
});
test('a recipient can agree before the progress display starts',async t=>{
  const {page,requests,suggestion,setGoals}=await fixture(t);
  setGoals([{id:2,title:'A week of steady studying',weekly:true,started:false,kind:'study_days',cooperative:false,creator:'hill',start_at:Date.now(),end_at:suggestion.expires_at,target:3,status:'waiting',progress:0,members:[{user:'hill',display:'Hill',status:'accepted',progress:0},{user:'cerro',display:'Cerro',status:'invited',progress:0}]}]);
  await page.reload();await page.getByText("You're invited",{exact:true}).waitFor();
  assert.equal(await page.locator('.challenge [role="progressbar"]').count(),0);
  await page.locator('[data-challenge-action="accept"]').click();await page.locator('.challenge [role="progressbar"]').waitFor();
  assert.deepEqual(requests.map(r=>r.body),[{action:'accept'}]);
  assert.match(await page.locator('.challenge-meta').innerText(),/Ends /);
});
test('a late invitation response cannot reconnect or repopulate a disconnected account',async t=>{
  const {page,control}=await fixture(t);
  let release!: ()=>void;control.hold=new Promise<void>(r=>release=r);
  await page.locator('[data-weekly-action="invite"]').click();
  await page.evaluate(()=>{window.ankiquestSession=null;window.dispatchEvent(new CustomEvent('ankiquest-auth'));});
  release();await page.waitForFunction(()=>!document.querySelector('.weekly-suggestion'));
  assert.equal(await page.locator('.challenge[data-goal]').count(),0);
});
test('the weekly card fits small and wide screens in both languages and themes',async t=>{
  for(const language of ['en-US','es-ES']) {
    const {page}=await fixture(t,{language});await page.locator('.weekly-suggestion').waitFor();
    for(const width of [320,390,1440])for(const theme of ['light','dark'] as const) {
      await page.setViewportSize({width,height:1000});await page.emulateMedia({colorScheme:theme});
      const box=await page.locator('.weekly-suggestion').evaluate(e=>({scroll:e.scrollWidth,width:e.clientWidth}));
      assert.ok(box.scroll<=box.width+1,`${language} ${width} ${theme} card overflow`);
    }
  }
});
