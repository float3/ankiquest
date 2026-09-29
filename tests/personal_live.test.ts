// End-to-end smoke test for private personal pages against the compiled server.
// ANKIQUEST_BIN=/path/to/ankiquest PLAYWRIGHT_MODULE=/path/to/playwright node --experimental-strip-types tests/personal_live.test.ts
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import net from 'node:net';
import {spawn} from 'node:child_process';
import type {ChildProcess} from 'node:child_process';
import type {Browser} from 'playwright';
import {chromium, root as repo} from './support/web.ts';

const exe = process.env.ANKIQUEST_BIN || path.join(repo, 'target', 'debug', process.platform === 'win32' ? 'ankiquest.exe' : 'ankiquest');
const run = fs.mkdtempSync(path.join(os.tmpdir(), 'ankiquest-personal-live-'));
const token = 'alice-personal-test-token';
const password = 'members-personal-test-password';
let server: ChildProcess | undefined, browser: Browser | undefined, log: number | undefined;

async function freePort() {
  return new Promise<number>((resolve, reject) => {
    const socket = net.createServer();
    socket.once('error', reject);
    socket.listen(0, '127.0.0.1', () => {
      const port = (socket.address() as net.AddressInfo).port;
      socket.close(() => resolve(port));
    });
  });
}

async function main() {
  const port = await freePort();
  const base = `http://127.0.0.1:${port}`;
  const passwordFile = path.join(run, 'password');
  fs.writeFileSync(passwordFile, password);
  fs.writeFileSync(path.join(run, 'config.json'), JSON.stringify({
    addr:`127.0.0.1:${port}`, state_dir:run, private_site:true, site_password_file:passwordFile,
    week_timezone:'UTC', week_rollover_hour:4,
    users:{alice:{display:'Alice',token}},
  }));
  log = fs.openSync(path.join(run, 'server.log'), 'a');
  const child = server = spawn(exe, [path.join(run, 'config.json')], {windowsHide:true,stdio:['ignore',log,log]});
  for (let i=0;i<100;i++) {
    if (child.exitCode !== null) throw Error(fs.readFileSync(path.join(run, 'server.log'), 'utf8'));
    try { if ((await fetch(base + '/auth/status')).ok) break; } catch {}
    if (i===99) throw Error('Server did not start');
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  assert.equal((await fetch(base + '/api/study/alice')).status, 401);

  browser = await chromium.launch({headless:true,...(process.env.PLAYWRIGHT_CHANNEL ? {channel:process.env.PLAYWRIGHT_CHANNEL} : {})});
  const context = await browser.newContext({locale:'en-US',viewport:{width:390,height:844}});
  const page = await context.newPage();
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(base + '/login?next=%2Ftoday');
  await page.locator('#password').fill(password);
  await page.locator('button[type=submit]').click();
  await page.waitForURL('**/today');
  await page.locator('#personal-gate:visible').waitFor();
  assert.equal((await page.request.get(base + '/api/study/alice')).status(), 401);
  await page.locator('#personal-connect [name=user]').fill('alice');
  await page.locator('#personal-connect [name=token]').fill(token);
  await page.locator('#personal-connect button').click();
  try {await page.getByRole('heading', {name:'Latest synced session'}).waitFor({timeout:5000});}
  catch (cause) {throw Error(`Today did not load: ${(await page.locator('body').innerText()).slice(0,1500)}; page errors: ${errors.join('; ')}`,{cause});}
  assert.equal(await page.locator('#personal-gate:visible').count(), 0);

  const now = Date.now();
  const reviews = [
    {id:now-120000,cid:1,last_ivl:0,time_ms:30000,kind:0},
    {id:now-60000,cid:2,last_ivl:1,time_ms:30000,kind:1},
  ];
  const uploaded = await fetch(base + '/api/reviews/alice', {
    method:'POST', headers:{Authorization:`Bearer ${token}`,'Content-Type':'application/json'},
    body:JSON.stringify({reviews,clock:{offset_west_min:0,rollover_hour:4},silent:false}),
  });
  assert.equal(uploaded.status, 200, await uploaded.text());
  await page.reload();
  await page.getByRole('heading', {name:'Latest synced session'}).waitFor();
  await page.locator('.personal-summary strong').first().waitFor();
  assert.match((await page.locator('.personal-stamp').textContent())!, /Study data updated/);
  assert.equal(await page.locator('.personal-summary strong').first().textContent(), '2');
  await page.goto(base + '/history');
  await page.locator('[data-day]:visible').first().waitFor();
  await page.locator('[data-day]:visible').first().click();
  assert.match((await page.locator('#day-detail').textContent())!, /reviews/);
  await page.goto(base + '/settings');
  if (await page.locator('#freeze-toggle').isChecked()) {
    await page.locator('#freeze-toggle').uncheck();
    await page.locator('#freeze-status').filter({hasText:'Saved'}).waitFor();
  }
  await page.locator('#freeze-toggle').check();
  await page.locator('#freeze-status').filter({hasText:'Saved'}).waitFor();
  const freeze = await page.request.get(base + '/api/streak-freezes/alice');
  assert.equal(freeze.status(), 200);
  assert.equal((await freeze.json() as {enabled: boolean}).enabled, true);
  assert.deepEqual(errors, []);
  assert((await page.evaluate(() => document.documentElement.scrollWidth)) <= 390);
  await context.close();
  console.log('PASS: private server gate, owner connection, upload freshness, recap, day detail, and settings save.');
}

main().catch(error => {console.error(error);process.exitCode=1;}).finally(async () => {
  if (browser) await browser.close();
  const child = server;
  if (child && child.exitCode === null) {child.kill();await new Promise(resolve => child.once('exit', resolve));}
  if (log !== undefined) fs.closeSync(log);
  fs.rmSync(run, {recursive:true,force:true});
});
