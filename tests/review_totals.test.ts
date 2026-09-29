import assert from 'node:assert/strict';
import {test} from 'node:test';
import {addSite, chromium, open, page as built, prefix, script} from './support/web.ts';
test('Overview and Year review keep identical numeric review totals in English and Spanish',async()=>{
 const browser=await chromium.launch({headless:true,...(process.env.PLAYWRIGHT_CHANNEL?{channel:process.env.PLAYWRIGHT_CHANNEL}:process.platform==='win32'?{channel:'msedge'}:{})});
 try {
  const html=built('community');
  // Everything before the page starts rendering.
  const source=prefix('pages/community','// Listeners above are registered right away');
  for(const locale of ['en-US','es-ES']){
   const page=await browser.newPage({locale});
   await open(page,html.replace(/<script\b[^>]*>[\s\S]*?<\/script>/g,''));
   await addSite(page);
   await page.addScriptTag({content:script('avatars')});
   await page.addScriptTag({content:source});
   const totals=await page.evaluate(()=>{
    state.data={meta:{},players:[{user:'cerro',display:'Cerro',trophies:[],monthly:[],year_review:{reviews:1234,xp:9000,study_days:40,best_streak:12}}],awards:[],calendar:[],weeks:[],seasons:[],records:[],head_to_head:[]};
    renderOverview();renderYear();
    return ['view-overview','view-year'].map(id=>document.querySelectorAll('#'+id+' .kpi')[id==='view-overview'?3:1]!.querySelector('strong')!.textContent!.replace(/[^0-9]/g,''));
   });
   assert.deepEqual(totals,['1234','1234'],locale);await page.close();
  }
 } finally {await browser.close();}
});
