// Exercise the real leaderboard renderer in both supported languages and themes.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {addSite, chromium, open, page as built, prefix, root, script} from './support/web.ts';
const html = built('index');
const renderer = prefix('pages/index', 'async function render(');
const styles = [...html.matchAll(/<style\b[^>]*>([\s\S]*?)<\/style>/g)].map(match => match[1]).join('\n');
const scaffold = html.match(/<body\b[^>]*>([\s\S]*?)<script>/)![1];

(async () => {
  const browser = await chromium.launch({headless:true,
    ...(process.env.PLAYWRIGHT_CHANNEL ? {channel:process.env.PLAYWRIGHT_CHANNEL} : process.platform==='win32' ? {channel:'msedge'} : {}),
  });
  let checks = 0;
  try {
    for (const locale of ['en-US','es-ES']) for (const width of [320,390,1440]) for (const colorScheme of ['light','dark'] as const) {
      const page = await browser.newPage({locale, viewport:{width,height:800}, colorScheme});
      await open(page, `<html><head><style>${fs.readFileSync(path.join(root,'static/site.css'),'utf8')}\n${fs.readFileSync(path.join(root,'static/avatars.css'),'utf8')}\n${styles}</style></head><body>${scaffold}</body></html>`);
      await page.addScriptTag({content:script('avatars')});
      await addSite(page);
      await page.addScriptTag({content:renderer});
      await page.evaluate(() => {
        const rows = ['studied','pending','protected'].map((streak_state,index) => ({user:streak_state,display:'Friend '+index,xp:100-index,level:1,streak:27,today_reviews:index===0?1:0,streak_state,day_ends_at:Date.now()+3600000}));
        app.innerHTML = board(rows,'me');
      });
      const states = await page.locator('.board .streak').evaluateAll((elements: HTMLElement[]) => elements.map(element => ({state:element.dataset.streakState,label:element.getAttribute('aria-label')!,title:element.title,text:element.textContent,color:getComputedStyle(element).color,svg:!!element.querySelector('svg')})));
      assert.deepEqual(states.map(item=>item.state), ['studied','pending','protected']);
      assert.equal(new Set(states.map(item=>item.color)).size,3);
      for (const item of states) { assert(item.svg); assert(item.text.includes('27')); assert(item.label.includes('27')); assert.equal(item.label,item.title); }
      assert(states[0]!.label.includes(locale==='es-ES'?'Estudió hoy':'Studied today'));
      assert(states[1]!.label.includes(locale==='es-ES'?'Aún no ha estudiado':'Not studied yet'));
      assert(states[2]!.label.includes(locale==='es-ES'?'Racha protegida':'Streak protected'));
      assert(states[2]!.label.includes(locale==='es-ES'?'Aún no ha estudiado':'Not studied yet'));
      assert.equal(await page.locator('.streak[data-streak-state="pending"] .streak-clock').count(),1);
      assert.equal(await page.locator('.streak[data-streak-state="protected"] .streak-snowflake').count(),1);
      assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
      if (process.env.ANKIQUEST_STREAK_EVIDENCE && width===390 && colorScheme==='dark' && locale==='en-US') {
        await page.screenshot({path:process.env.ANKIQUEST_STREAK_EVIDENCE,fullPage:true});
      }
      // Old servers, an expired cached study day, and zero streaks must not claim activity.
      await page.evaluate(() => {
        const rows = [{user:'old',display:'Old',streak:3,today_reviews:0},{user:'expired',display:'Expired',streak:4,today_reviews:2,streak_state:'studied',day_ends_at:Date.now()-1},{user:'none',display:'None',streak:0,today_reviews:0}].map(row=>({...row,xp:100,level:1}));
        app.innerHTML=board(rows,'me');
      });
      assert.equal(await page.locator('.streak[data-streak-state="pending"]').count(),1);
      assert.equal(await page.locator('.streak[data-streak-state="unknown"]').count(),1);
      assert.equal(await page.locator('.streak').count(),2);
      checks++;
      await page.close();
    }
  } finally { await browser.close(); }
  console.log(`PASS: ${checks} English/Spanish streak-status layouts, accessibility, old-server and rollover cases.`);
})().catch(error=>{console.error(error);process.exitCode=1;});
