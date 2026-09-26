// Uses the real leaderboard/profile renderers and CSS without contacting a server.
// PLAYWRIGHT_MODULE, PLAYWRIGHT_CHANNEL, ANKIQUEST_AVATAR_EVIDENCE are optional.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const root = path.resolve(__dirname, '..');
const html = fs.readFileSync(path.join(root, 'static/index.html'), 'utf8');
const styles = [...html.matchAll(/<style\b[^>]*>([\s\S]*?)<\/style>/g)].map(match => match[1]).join('\n');
const source = [...html.matchAll(/<script\b[^>]*>([\s\S]*?)<\/script>/g)].map(match => match[1]).find(script => script.includes('function board('));
const renderer = source.slice(0, source.indexOf('async function render()'));
const sharedStyles = fs.readFileSync(path.join(root, 'static/avatars.css'), 'utf8');
const sharedScript = fs.readFileSync(path.join(root, 'static/avatars.js'), 'utf8');
const siteStyles = fs.readFileSync(path.join(root, 'static/site.css'), 'utf8');
const siteScript = require('./site_assets.cjs').siteScript();
const scaffold = html.match(/<body\b[^>]*>([\s\S]*?)<script>/)[1];
const evidence = process.env.ANKIQUEST_AVATAR_EVIDENCE;
if (evidence) fs.mkdirSync(evidence, { recursive: true });

async function main() {
  const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHANNEL ? { channel: process.env.PLAYWRIGHT_CHANNEL } : {}) });
  const results = [], failures = [];
  try {
    for (const width of [320, 390, 1440]) for (const colorScheme of ['light', 'dark']) {
      const page = await browser.newPage({ locale:"en-US", viewport: { width, height: 1100 }, colorScheme });
      await page.route('**/*', route => route.abort());
      await page.setContent(`<!doctype html><html><head><meta charset="utf-8"><style>${siteStyles}</style><style>${styles}</style><style>${sharedStyles}</style></head><body>${scaffold}</body></html>`);
      await page.addScriptTag({ content: sharedScript });
      await page.addScriptTag({ content: siteScript });
      await page.addScriptTag({ content: renderer });
      await page.evaluate(() => {
        const names = ['Cerro', 'Alice Brown', 'Alice & <team>'];
        const rows = names.map((display, index) => ({ user: 'qa-player-' + index, display, xp: 12500 - 1000 * index, week_xp: 12500 - 1000 * index, streak: 100, level: 20, today_reviews: 150 }));
        let content = `<section class="card"><h2>Leaderboard</h2>${board(rows, rows[0].user)}</section>`;
        for (const row of rows) {
          const profile = { ...row, achievements: [], quests: [], heatmap: [{ date: '2026-09-22', reviews: 150, xp: 1500 }], today: { reviews: 150, xp: 1500 }, lifetime: { reviews: 12345, hours: 150, best_streak: 100, best_combo: 75, days_active: 100 }, xp_into_level: 250, xp_for_next: 2000, xp_total: 145000 };
          const wrapper = document.createElement('div'); wrapper.innerHTML = view(profile, rows);
          content += wrapper.querySelector('.profile-hero').outerHTML;
        }
        app.innerHTML = content;
      });
      await page.evaluate(() => document.fonts.ready);
      const measurement = await page.evaluate(() => ({
        pageWidth: innerWidth, scrollWidth: document.documentElement.scrollWidth,
        names: [...document.querySelectorAll('.board .player-cell, .profile-hero .avatar-name')].map(container => {
          const isBoard = !!container.closest('.board');
          const bounds = container.getBoundingClientRect(), avatar = container.querySelector('.avatar').getBoundingClientRect(), text = isBoard ? container.querySelector('.name') : container.lastElementChild;
          const textBounds = text.getBoundingClientRect(), range = document.createRange(); range.selectNodeContents(text);
          const letters = range.getBoundingClientRect(), style = getComputedStyle(text);
          return { context: isBoard ? 'board' : 'profile', text: text.textContent, width: bounds.width, textWidth: textBounds.width, avatarCount: container.querySelectorAll('.avatar').length, avatarWidth: avatar.width, avatarVisible: avatar.left >= bounds.left - 0.5 && avatar.right <= bounds.right + 0.5, textVisible: letters.left >= textBounds.left - 0.5 && letters.right <= textBounds.right + 0.5, textOverflow: style.textOverflow, overflow: style.overflow, whiteSpace: style.whiteSpace };
        }),
      }));
      results.push({ width, colorScheme, ...measurement });
      try { assert(measurement.scrollWidth <= width, `${width}px ${colorScheme}: page overflow ${measurement.scrollWidth}`); } catch (error) { failures.push(error.message); }
      assert.equal(measurement.names.length, 6);
      for (const name of measurement.names) {
        const label = `${width}px ${colorScheme} ${name.context} ${name.text}`;
        try {
          assert(name.avatarVisible, `${label}: avatar is clipped by its name link (${name.width.toFixed(1)}px available for ${name.avatarWidth}px avatar)`);
          assert.equal(name.avatarCount, 1, `${label}: exactly one avatar is rendered`);
          assert.equal(name.avatarWidth, name.context === 'profile' ? 48 : width <= 620 ? 30 : 36, `${label}: avatar retains the theme's intended size`);
          assert(name.textWidth >= 40, `${label}: no usable space remains for the name`);
          assert(name.textVisible || (name.textOverflow === 'ellipsis' && name.overflow === 'hidden'), `${label}: name is silently clipped without ellipsis`);
        } catch (error) { failures.push(error.message); }
      }
      if (evidence) await page.screenshot({ path: path.join(evidence, `avatar-index-${width}-${colorScheme}.png`), fullPage: true });
      await page.close();
    }
  } finally { await browser.close(); }
  if (evidence) fs.writeFileSync(path.join(evidence, 'avatar-index-results.json'), JSON.stringify({ results, failures }, null, 2));
  assert.equal(failures.length, 0, failures.join('\n'));
  console.log('PASS: 36 leaderboard/profile avatar labels remain readable at 320/390/1440px, light/dark.');
}
main().catch(error => { console.error(error.message); process.exitCode = 1; });
