// Run with Node and Playwright installed. Optional environment variables:
// PLAYWRIGHT_MODULE: module name/path; PLAYWRIGHT_CHANNEL: e.g. msedge;
// ANKIQUEST_AVATAR_EVIDENCE: directory for screenshots and measurements.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');

const repository = path.resolve(__dirname, '..');
const source = fs.readFileSync(path.join(repository, 'static/community.html'), 'utf8');
const siteStyles = fs.readFileSync(path.join(repository, 'static/site.css'), 'utf8');
const siteScript = fs.readFileSync(path.join(repository, 'static/site.js'), 'utf8');
const styles = siteStyles + '\n' + [...source.matchAll(/<style\b[^>]*>([\s\S]*?)<\/style>/g)].map(match => match[1]).join('\n');
const sharedStylePath = path.join(repository, 'static/avatars.css');
const sharedScriptPath = path.join(repository, 'static/avatars.js');
const sharedStyles = fs.existsSync(sharedStylePath) ? fs.readFileSync(sharedStylePath, 'utf8') : '';
const sharedScript = fs.existsSync(sharedScriptPath) ? fs.readFileSync(sharedScriptPath, 'utf8') : '';
const script = [...source.matchAll(/<script\b[^>]*>([\s\S]*?)<\/script>/g)]
  .map(match => match[1]).find(value => value.includes('function avatar('));
assert(styles && script, 'Load the real community stylesheet and avatar renderer');
const rendererEnd = script.indexOf('function empty(');
assert(rendererEnd > script.indexOf('function avatar('), 'Avatar helpers precede page startup');
const renderer = script.slice(0, rendererEnd);
const evidence = process.env.ANKIQUEST_AVATAR_EVIDENCE;
if (evidence) fs.mkdirSync(evidence, { recursive: true });

async function main() {
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.PLAYWRIGHT_CHANNEL ? { channel: process.env.PLAYWRIGHT_CHANNEL } : {}),
  });
  const results = [], failures = [];
  try {
    for (const width of [390, 1440]) {
      for (const colorScheme of ['light', 'dark']) {
        const page = await browser.newPage();
        // The fixture contains synthetic users and must never contact a real server.
        await page.route('**/*', route => route.abort());
        await page.setViewportSize({ width, height: 1000 });
        await page.emulateMedia({ colorScheme });
        await page.setContent(`<!doctype html><html><head><meta charset="utf-8"><style>${styles}</style><style>${sharedStyles}</style></head><body><div id="fixture" class="shell stack"></div></body></html>`);
        if (sharedScript) await page.addScriptTag({ content: sharedScript });
        await page.addScriptTag({ content: siteScript });
        await page.addScriptTag({ content: renderer });
        await page.evaluate(() => {
          const single = avatar('qa-cerro', 'Cerro');
          const double = avatar('qa-alice', 'Alice Brown');
          document.getElementById('fixture').innerHTML = `
            <article class="card" data-context="matchup"><div class="matchup">
              <div class="side">${single}<strong>171</strong><span>Cerro</span><small>daily wins</small></div>
              <span class="vs">VS</span>
              <div class="side">${double}<strong>16</strong><span>Alice Brown</span><small>daily wins</small></div>
            </div></article>
            <article class="card" data-context="awards"><ul class="list"><li class="list-row">
              ${single}<div class="row-text"><strong>Welcome back after 7 days</strong><small>Cerro · 4 Jul 2024</small></div>
            </li></ul></article>
            <article class="card" data-context="standings"><ul class="list"><li class="list-row">
              ${double}<div class="row-text"><strong>1. Alice Brown</strong><div class="progress"><i style="width:75%"></i></div></div>
              <div class="row-end"><strong>100 XP</strong><small>20 reviews</small></div>
            </li></ul></article>
            <article class="card" data-context="table"><table><tbody><tr><td>
              <span class="table-player">${double}<button type="button">Alice Brown</button></span>
            </td></tr></tbody></table></article>
            <article class="card calendar-card" data-context="calendar"><div class="calendar"><button type="button" class="day">
              <span class="date">22</span><span class="day-bottom">${single}<span class="winner">Cerro</span></span><span class="mini-status">Final</span>
            </button></div></article>
            <article class="card" data-context="challenge"><div class="challenge"><div class="members"><div class="member">
              ${double}<span>Alice Brown</span><small>10 reviews</small>
            </div></div></div></article>`;
        });
        await page.evaluate(() => document.fonts.ready);
        const measurements = await page.locator('.avatar').evaluateAll(avatars => avatars.map(element => {
          const bounds = element.getBoundingClientRect();
          const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
          const text = [];
          for (let node = walker.nextNode(); node; node = walker.nextNode()) {
            if (node.textContent.trim()) text.push(node);
          }
          const range = document.createRange();
          if (text.length) {
            range.setStart(text[0], 0);
            range.setEnd(text[text.length - 1], text[text.length - 1].length);
          }
          const letters = range.getBoundingClientRect();
          const style = getComputedStyle(element);
          return {
            context: element.closest('[data-context]').dataset.context,
            initials: element.textContent.trim(),
            width: bounds.width, height: bounds.height,
            letterWidth: letters.width, letterHeight: letters.height,
            offsetX: letters.x + letters.width / 2 - bounds.x - bounds.width / 2,
            offsetY: letters.y + letters.height / 2 - bounds.y - bounds.height / 2,
            display: style.display,
          };
        }));
        assert.equal(measurements.length, 7, 'Every relevant avatar context is rendered');
        for (const measurement of measurements) {
          const label = `${width}px ${colorScheme} ${measurement.context} ${measurement.initials}`;
          results.push({ width, colorScheme, ...measurement });
          try {
            assert(measurement.width > 0 && measurement.height > 0, `${label}: avatar is visible`);
            assert(Math.abs(measurement.width - measurement.height) <= 0.5, `${label}: avatar stays square`);
            const expectedSize = { matchup: 48, awards: 36, standings: 36, table: 29, calendar: width < 621 ? 21 : 24, challenge: 24 }[measurement.context];
            assert.equal(measurement.width, expectedSize, `${label}: existing context size is preserved`);
            assert(measurement.letterWidth > 0 && measurement.letterHeight > 0, `${label}: initials are visible`);
            assert(Math.abs(measurement.offsetX) <= 1.5, `${label}: horizontal offset ${measurement.offsetX.toFixed(2)}px`);
            // Font ascent/descent can shift the glyph box slightly within a centered line.
            assert(Math.abs(measurement.offsetY) <= 2.5, `${label}: vertical offset ${measurement.offsetY.toFixed(2)}px (display: ${measurement.display})`);
          } catch (error) {
            failures.push(error.message);
          }
        }
        if (evidence) await page.screenshot({ path: path.join(evidence, `avatar-layout-${width}-${colorScheme}.png`), fullPage: true });
        await page.close();
      }
    }
  } finally {
    await browser.close();
  }
  if (evidence) fs.writeFileSync(path.join(evidence, 'avatar-layout-results.json'), JSON.stringify({ results, failures }, null, 2));
  assert.equal(failures.length, 0, failures.join('\n'));
  console.log(`PASS: ${results.length} avatar layouts centered across mobile/desktop and light/dark.`);
}

main().catch(error => { console.error(error.message); process.exitCode = 1; });
