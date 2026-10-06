// Authored labels are translated before inserting names, deck titles or messages.
import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs';
import path from 'node:path';
import type {I18n} from '../web/i18n.ts';
import {catalog as load, hostGlobals, root, run} from './support/web.ts';
const catalog = load('es'), french = load('fr'), german = load('de'), portuguese = load('pt');
interface Fixture {
  AnkiQuestI18n: I18n; AnkiQuestSite?: {member(): unknown}; ankiquestSession?: unknown; ankiquestLanguage?: string;
  fetch(input: string, options?: {headers?: Record<string, string>}): Promise<unknown>;
  CustomEvent: new (type: string, options?: {detail?: unknown}) => {type: string; detail: unknown};
  dispatchEvent(event: {type: string}): void;
}
type Handler = (event: {type: string}) => void;
// Lets the translation module finish work it started from an event.
const settle = () => new Promise(resolve => setImmediate(resolve));
function fixture(language='es', blocked=false) {
  const events: Record<string, Handler[]>={}, requests: [unknown, RequestInit | undefined][]=[], storage=new Map<string, string>(); let reloads=0;
  const context = {...hostGlobals(), navigator:{language}, location:{href:'https://quest.test/community',origin:'https://quest.test',reload(){reloads++;}},
    CustomEvent:class {type: string; detail: unknown; constructor(type: string,options?: {detail?: unknown}){this.type=type;this.detail=options?.detail;}},
    sessionStorage:{getItem:(key: string)=>storage.get(key) ?? null,setItem(key: string,value: string){if(blocked)throw Error('blocked');storage.set(key,value);}},
    document:{readyState:'loading',documentElement:{},querySelectorAll(){return [];},addEventListener(){}},
    addEventListener(type: string,handler: Handler){(events[type] ||= []).push(handler);}, dispatchEvent(event: {type: string}){for(const handler of events[event.type]||[])handler(event);},
    fetch:async(input: unknown,options?: RequestInit)=>{requests.push([input,options]);return {ok:true};}};
  run('i18n', context);
  return {context: context as unknown as Fixture,requests,storage,get reloads(){return reloads;}};
}
test('Spanish tagged labels preserve authored substitutions and placeholder-like names',async()=>{
  const {context}=fixture();await context.AnkiQuestI18n.ready;const {t,html}=context.AnkiQuestI18n;
  assert.equal(t('Friends'),'Amigos');
  assert.equal(t`Reply to ${'Friends::{0}'}`, 'Responder a Friends::{0}');
  assert.equal(html`<h2>Friends</h2><p>${'Friends::{0} Good job!'}</p>`,'<h2>Amigos</h2><p>Friends::{0} Good job!</p>');
  assert.equal(html`<input placeholder="Say something kind" value="${'Friends'}">`,'<input placeholder="Di algo amable" value="Friends">');
  assert.equal(t`Review ${15} cards`,'Repasa 15 tarjetas');
});
test('French, German and Portuguese follow Anki language tags and preserve names',async()=>{
  for(const [tag,label] of [['fr-FR','Amis'],['de-DE','Freunde'],['pt-BR','Amigos']] as const){
    const {context}=fixture(tag);await context.AnkiQuestI18n.ready;
    assert.equal(context.AnkiQuestI18n.language,tag.slice(0,2));
    assert.equal(context.AnkiQuestI18n.t('Friends'),label);
    assert.ok(context.AnkiQuestI18n.t`Reply to ${'Friends::{0}'}`.includes('Friends::{0}'));
  }
});

test('new personal pages use translated settings, streak and review terms',async()=>{
  for(const [tag,settings,streak,reviews] of [
    ['fr-FR','Réglages','Série en cours','Aucune révision envoyée pour le moment.'],
    ['de-DE','Einstellungen','Aktuelle Lernserie','Noch keine Wiederholungen hochgeladen.'],
    ['pt-PT','Definições','Sequência atual','Ainda não foram enviadas revisões.'],
  ] as const){
    const {context}=fixture(tag);await context.AnkiQuestI18n.ready;
    assert.equal(context.AnkiQuestI18n.t('Settings'),settings);
    assert.equal(context.AnkiQuestI18n.t('Current streak'),streak);
    assert.equal(context.AnkiQuestI18n.t('No reviews uploaded yet.'),reviews);
  }
});
test('new catalogs cover Spanish source phrases and preserve every substitution slot',()=>{
  const slots=(value: string)=>[...value.matchAll(/\{\d+\}/g)].map(match=>match[0]).sort();
  for(const [language,values] of Object.entries({fr:french,de:german,pt:portuguese})){
    for(const source of Object.keys(catalog)){
      assert.ok(Object.hasOwn(values,source),`${language} missing ${source}`);
      assert.deepEqual(slots(values[source]!),slots(source),`${language}: ${source}`);
    }
  }
});

test('the new crop editor file hint has a Spanish translation',async()=>{
  const {context}=fixture();await context.AnkiQuestI18n.ready;
  assert.equal(context.AnkiQuestI18n.html`<p>JPEG or PNG, up to 20 MB.</p>`, '<p>JPEG o PNG, hasta 20 MB.</p>');
});
test('waiting custom challenges and a singular streak freeze read naturally in Spanish',async()=>{
  const {context}=fixture();await context.AnkiQuestI18n.ready;const {t}=context.AnkiQuestI18n;
  assert.equal(t('Waiting for everyone'),'Esperando a todos');
  assert.equal(t('Start when everyone accepts'),'Empezar cuando todos acepten');
  assert.equal(t`A freeze covered ${'Monday'}. You have ${1} freeze left.`,
    'Un protector cubrió el Monday. Te queda 1 protector.');
});
test('authored labels and substitution slots have Spanish catalog entries',()=>{
  const missing: string[]=[];
  const sources=[
    ...fs.readdirSync(path.join(root,'web'),{recursive:true,encoding:'utf8'}).filter(name=>name.endsWith('.ts')).map(name=>path.join('web',name)),
    ...['community.html','index.html','personal.html','conquered.html'].map(name=>path.join('static',name)),
  ];
  for(const name of sources){
    const source=fs.readFileSync(path.join(root,name),'utf8');
    for(const match of source.matchAll(/\baqText\(\s*(['"])(.*?)\1\s*\)/gs)){
      if(!Object.hasOwn(catalog,match[2]!.trim()))missing.push(`${name}: ${match[2]}`);
    }
    for(const match of source.matchAll(/\bdata-i18n="([^"]*)"/g)){
      if(!Object.hasOwn(catalog,match[1]!))missing.push(`${name}: ${match[1]}`);
    }
  }
  assert.deepEqual(missing,[]);
  const slots=(value: string)=>[...value.matchAll(/\{\d+\}/g)].map(match=>match[0]).sort();
  for(const [source,target] of Object.entries(catalog))assert.deepEqual(slots(target),slots(source),source);
});
test('language headers go only to same-origin API requests and preserve authentication',async()=>{
  const {context,requests}=fixture();
  await context.fetch('/api/profile/cerro',{headers:{Authorization:'Bearer private'}});
  const headers=requests[0]![1]!.headers as Headers; assert.equal(headers.get('Accept-Language'),'es');assert.equal(headers.get('Authorization'),'Bearer private');
  await context.fetch('https://other.test/api/profile/cerro');assert.equal(requests[1]![1]?.headers,undefined);
});
test('late native handoff rebuilds labels once and stores only language',async()=>{
  const f=fixture('en-US');await f.context.AnkiQuestI18n.ready;f.context.ankiquestLanguage='es-AR';
  f.context.dispatchEvent(new f.context.CustomEvent('ankiquest-auth'));
  f.context.dispatchEvent(new f.context.CustomEvent('ankiquest-auth'));
  assert.equal(f.context.AnkiQuestI18n.language,'es');assert.equal(f.reloads,1);
  assert.deepEqual([...f.storage],[['ankiquestLanguage','es']]);
});
test('blocked storage cannot produce a native handoff reload loop',async()=>{
  const f=fixture('en',true);await f.context.AnkiQuestI18n.ready;f.context.AnkiQuestI18n.set('es');f.context.AnkiQuestI18n.set('es');
  assert.equal(f.reloads,0);assert.equal(f.context.AnkiQuestI18n.t('Friends'),'Amigos');
});
test('language persistence does not read or change account identity',async()=>{
  const f=fixture();let calls=0;f.context.AnkiQuestSite={member(){calls++;throw Error('must not reconcile');}};
  f.context.ankiquestSession=null;f.context.dispatchEvent(new f.context.CustomEvent('ankiquest-auth'));
  await f.context.AnkiQuestI18n.ready;await settle();
  assert.equal(calls,0);assert.equal(f.requests.length,0);
  f.context.dispatchEvent(new f.context.CustomEvent('ankiquest:identity',{detail:{user:'cerro'}}));
  await settle();
  assert.equal(f.requests.length,1);assert.equal(f.requests[0]![0],'/api/language/cerro');
  f.context.dispatchEvent(new f.context.CustomEvent('ankiquest:identity',{detail:{user:null}}));
  await settle();
  assert.equal(f.requests.length,1);
});
