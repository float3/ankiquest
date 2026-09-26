// Authored labels are translated before inserting names, deck titles or messages.
const assert = require('node:assert/strict');
const {test} = require('node:test');
const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const catalog = JSON.parse(fs.readFileSync(path.join(__dirname,'../static/translations-es.json'),'utf8'));
const script = fs.readFileSync(path.join(__dirname,'../static/i18n.js'),'utf8');
function fixture(language='es', blocked=false) {
  const events={}, requests=[], storage=new Map(); let reloads=0;
  const context = {URL, Request, Headers, navigator:{language}, location:{href:'https://quest.test/community',origin:'https://quest.test',reload(){reloads++;}},
    CustomEvent:class {constructor(type,options){this.type=type;this.detail=options?.detail;}},
    sessionStorage:{getItem:key=>storage.get(key),setItem(key,value){if(blocked)throw Error('blocked');storage.set(key,value);}},
    document:{readyState:'loading',documentElement:{},querySelectorAll(){return [];},addEventListener(){}},
    addEventListener(type,handler){(events[type] ||= []).push(handler);}, dispatchEvent(event){for(const handler of events[event.type]||[])handler(event);},
    fetch:async(input,options)=>{requests.push([input,options]);return {ok:true};}, AnkiQuestSpanish:catalog};
  context.window=context; vm.runInNewContext(script,context);
  return {context,requests,storage,get reloads(){return reloads;}};
}
test('Spanish tagged labels preserve authored substitutions and placeholder-like names',()=>{
  const {context}=fixture();const {t,html}=context.AnkiQuestI18n;
  assert.equal(t('Friends'),'Amigos');
  assert.equal(t`Reply to ${'Friends::{0}'}`, 'Responder a Friends::{0}');
  assert.equal(html`<h2>Friends</h2><p>${'Friends::{0} Good job!'}</p>`,'<h2>Amigos</h2><p>Friends::{0} Good job!</p>');
  assert.equal(html`<input placeholder="Say something kind" value="${'Friends'}">`,'<input placeholder="Di algo amable" value="Friends">');
  assert.equal(t`Review ${15} cards`,'Repasa 15 tarjetas');
});

test('the new crop editor file hint has a Spanish translation',()=>{
  const {context}=fixture();
  assert.equal(context.AnkiQuestI18n.html`<p>JPEG or PNG, up to 20 MB.</p>`, '<p>JPEG o PNG, hasta 20 MB.</p>');
});
test('language headers go only to same-origin API requests and preserve authentication',async()=>{
  const {context,requests}=fixture();
  await context.fetch('/api/profile/cerro',{headers:{Authorization:'Bearer private'}});
  const headers=requests[0][1].headers; assert.equal(headers.get('Accept-Language'),'es');assert.equal(headers.get('Authorization'),'Bearer private');
  await context.fetch('https://other.test/api/profile/cerro');assert.equal(requests[1][1].headers,undefined);
});
test('late native handoff rebuilds labels once and stores only language',()=>{
  const f=fixture('en-US');f.context.ankiquestLanguage='es-AR';
  f.context.dispatchEvent(new f.context.CustomEvent('ankiquest-auth'));
  f.context.dispatchEvent(new f.context.CustomEvent('ankiquest-auth'));
  assert.equal(f.context.AnkiQuestI18n.language,'es');assert.equal(f.reloads,1);
  assert.deepEqual([...f.storage],[['ankiquestLanguage','es']]);
});
test('blocked storage cannot produce a native handoff reload loop',()=>{
  const f=fixture('en',true);f.context.AnkiQuestI18n.set('es');f.context.AnkiQuestI18n.set('es');
  assert.equal(f.reloads,0);assert.equal(f.context.AnkiQuestI18n.t('Friends'),'Amigos');
});
test('language persistence does not read or change account identity',async()=>{
  const f=fixture();let calls=0;f.context.AnkiQuestSite={member(){calls++;throw Error('must not reconcile');}};
  f.context.ankiquestSession=null;f.context.dispatchEvent(new f.context.CustomEvent('ankiquest-auth'));
  assert.equal(calls,0);assert.equal(f.requests.length,0);
  f.context.dispatchEvent(new f.context.CustomEvent('ankiquest:identity',{detail:{user:'cerro'}}));
  assert.equal(f.requests.length,1);assert.equal(f.requests[0][0],'/api/language/cerro');
  f.context.dispatchEvent(new f.context.CustomEvent('ankiquest:identity',{detail:{user:null}}));
  assert.equal(f.requests.length,1);
});
