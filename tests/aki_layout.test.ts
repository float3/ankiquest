// Real renderers, production artwork, and responsive styles; no member data or external server.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import type {Route} from 'playwright';
import {addSite, chromium, fulfillAsset, page as built, prefix, root, script} from './support/web.ts';
const html = built('index');
const styles = fs.readFileSync(path.join(root, 'static/site.css'), 'utf8') + '\n' + [...html.matchAll(/<style\b[^>]*>([\s\S]*?)<\/style>/g)].map(m => m[1]).join('\n');
const renderer = prefix('pages/index', 'async function render(');
const scaffold = html.match(/<body\b[^>]*>([\s\S]*?)<script>/)![1];
const evidence = process.env.ANKIQUEST_AKI_EVIDENCE;
if (evidence) fs.mkdirSync(evidence, {recursive:true});
async function assets(route: Route) {
  const url = new URL(route.request().url());
  if (url.pathname === '/week' && route.request().resourceType() === 'document') return route.fulfill({contentType:'text/html',body:`<!doctype html><html><head><style>${styles}</style></head><body>${scaffold}</body></html>`});
  if (url.pathname === '/auth/status') return route.fulfill({contentType:'application/json',body:JSON.stringify({private_site:false,authenticated:true,member:null})});
  if (url.pathname === '/api/avatars') return route.fulfill({contentType:'application/json',body:'{}'});
  if (url.pathname === '/i18n.wasm') return void await fulfillAsset(route, url.pathname);
  const name = url.pathname.match(/^\/aki\/(welcome|review|celebrate|streak|freeze|winner|face)\.png$/)?.[1];
  if (name) await route.fulfill({contentType:'image/png',body:fs.readFileSync(path.join(root,'static/aki',name+'.png'))});
  else await route.abort();
}
async function main() {
  const browser = await chromium.launch({headless:true,...(process.env.PLAYWRIGHT_CHANNEL?{channel:process.env.PLAYWRIGHT_CHANNEL}:{})});
  try {
    for (const width of [320,390,1440]) for (const colorScheme of ['light','dark'] as const) for (const locale of ['en-US','es-ES']) {
      const page = await browser.newPage({viewport:{width,height:1100},colorScheme,locale});
      const errors: string[]=[];page.on('pageerror',e=>errors.push(e.message));
      await page.route('**/*',assets);
      await page.goto('https://ankiquest.test/week');
      await addSite(page);
      await page.addScriptTag({content:script('avatars')});
      await page.addScriptTag({content:renderer});
      await page.evaluate(()=>document.dispatchEvent(new Event('DOMContentLoaded')));
      await page.evaluate(()=>AnkiQuestSite.status());
      await page.evaluate(() => {
        const p={user:'alice',display:'Alice',level:4,streak:3,streak_state:'studied',day_ends_at:Date.now()+86400000,today:{reviews:20,xp:200},lifetime:{reviews:180,hours:2,best_streak:3,best_combo:10,days_active:9},achievements:[],quests:[{title:'A little practice',progress:6,target:10,reward:20,done:false}],heatmap:[],xp_total:2400,xp_into_level:120,xp_for_next:250};
        app.innerHTML=view(p,[{...p,xp:2000,week_xp:2000,today_reviews:20}],true);
        AnkiQuestAki.decorate();AnkiQuestAki.decorate();
        document.querySelectorAll<HTMLImageElement>('.aki-art').forEach(img=>img.loading='eager');
      });
      await page.waitForFunction(()=>[...document.querySelectorAll<HTMLImageElement>('.aki-art')].every(img=>img.complete&&img.naturalWidth>0));
      assert.equal(await page.locator('[data-aki-companion="streak"] .aki-art').count(),1);
      assert.equal(await page.locator('.page-hero > .aki-art').count(),1,'repeat decoration is idempotent');
      assert.equal(await page.locator('.brand img').getAttribute('src'),'/aki/face.png');
      assert.equal(await page.locator('.brand img').evaluate(el=>getComputedStyle(el).width),'38px');
      assert.equal(await page.locator('.aki-art:not([alt=""])').count(),0);
      assert.equal(await page.locator('.aki-art[tabindex], .aki-art[role="button"]').count(),0);
      assert.equal(await page.locator('[data-aki-companion] p').innerText(),locale==='es-ES'?'Hoy estuviste presente. Ese progreso vale la pena.':'You showed up today. That is progress worth keeping.');
      for (const scale of [1,2]) {
        await page.evaluate(scale=>document.querySelector<HTMLElement>('[data-aki-companion] p')!.style.fontSize=(14*scale)+'px',scale);
        const dims=await page.evaluate(()=>({width:innerWidth,content:document.documentElement.scrollWidth}));
        assert(dims.content<=dims.width,`Aki profile overflow: ${width}px ${colorScheme} ${locale} text ${scale}: ${JSON.stringify(dims)}`);
      }
      await page.evaluate(()=>document.querySelector<HTMLElement>('[data-aki-companion] p')!.style.fontSize='');
      if(evidence&&locale==='en-US'&&width!==390) await page.screenshot({path:path.join(evidence,`aki-profile-${width}-${colorScheme}.png`),fullPage:true});
      assert.deepEqual(errors,[]);await page.close();
    }
    // The live observer handles asynchronously rendered content and reopened dialogs.
    const page=await browser.newPage({viewport:{width:320,height:740}});
    await page.route('**/*',assets);
    await page.setContent(`<html><head><base href="https://ankiquest.test/"><style>${styles}</style></head><body><dialog id="streak-freezes" class="freeze-settings"></dialog><main id="dynamic"></main></body></html>`);
    await page.addScriptTag({content:'window.AnkiQuestI18n={t:s=>s};\n'+script('aki')});
    await page.evaluate(()=>document.dispatchEvent(new Event('DOMContentLoaded')));
    for(let n=0;n<3;n++) {
      await page.evaluate(()=>{const dialog=document.querySelector<HTMLDialogElement>('#streak-freezes')!;dialog.innerHTML='<h3 id="freeze-title">Streak protection</h3><p>Your preference stays in control.</p>';dialog.showModal();});
      await page.locator('#streak-freezes .aki-dialog-art').waitFor();
      assert.equal(await page.locator('#streak-freezes .aki-art').count(),1,'exactly one freeze pose per opening');
      await page.evaluate(()=>document.querySelector<HTMLDialogElement>('#streak-freezes')!.close());
    }
    await page.evaluate(()=>document.querySelector('#dynamic')!.innerHTML='<div class="empty">No activity yet</div><section id="view-trophies"><div class="card-head"><h2>Your trophies</h2></div></section>');
    await page.locator('.empty .aki-art').waitFor();
    assert.equal(await page.locator('#view-trophies .aki-art').getAttribute('src'),'/aki/winner.png');
    if(evidence)await page.screenshot({path:path.join(evidence,'aki-empty-state.png'),fullPage:true});
    await page.close();
    console.log('PASS: Aki images, EN/ES, 320/390/1440px light/dark, 200% encouragement text, decorative accessibility, dynamic states and reopened dialogs.');
  } finally {await browser.close();}
}
main().catch(error=>{console.error(error);process.exitCode=1;});
