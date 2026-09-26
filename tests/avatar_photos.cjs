// Browser-only regression suite: all requests use synthetic users and mocked routes.
// Optional PLAYWRIGHT_MODULE and PLAYWRIGHT_CHANNEL work as in avatar_layout.cjs.
// ANKIQUEST_AVATAR_WIDTH and ANKIQUEST_AVATAR_THEME select a viewport/theme (390/light by default).
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const root = path.resolve(__dirname, '..');
const script = fs.readFileSync(path.join(root, 'static/avatars.js'), 'utf8');
const stylesheet = fs.readFileSync(path.join(root, 'static/avatars.css'), 'utf8');
const siteStyles = fs.readFileSync(path.join(root, 'static/site.css'), 'utf8');
const siteScript = require('./site_assets.cjs').siteScript();

async function main() {
  const browser = await chromium.launch({ headless: true, ...(process.env.PLAYWRIGHT_CHANNEL ? { channel: process.env.PLAYWRIGHT_CHANNEL } : {}) });
  const checks = [], requests = [], errors = [];
  const metadata = Object.create(null), broken = new Set();
  let image, imageRequests = 0, pauseWrite = null, pauseMetadata = null, pauseStatus = null, cookieUser = null;
  try {
    const page = await browser.newPage({ locale:"en-US", viewport: { width: Number(process.env.ANKIQUEST_AVATAR_WIDTH || 390), height: 844 }, colorScheme: process.env.ANKIQUEST_AVATAR_THEME || 'light' });
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(() => {
      window.ankiquestSession = { user: 'cerro', token: 'initial-native-token' };
      window.activePreviewURLs = new Set();
      const create = URL.createObjectURL.bind(URL), revoke = URL.revokeObjectURL.bind(URL);
      URL.createObjectURL = blob => { const url = create(blob); activePreviewURLs.add(url); return url; };
      URL.revokeObjectURL = url => { activePreviewURLs.delete(url); revoke(url); };
    });
    await page.route('**/*', async route => {
      const request = route.request(), url = new URL(request.url());
      assert.equal(url.origin, 'https://avatar.test', 'No external network requests');
      if (url.pathname === '/') return route.fulfill({ contentType: 'text/html', body: '<!doctype html><html><head><meta charset="utf-8"><link rel="stylesheet" href="/site.css"><link rel="stylesheet" href="/avatars.css"><script src="/site.js" defer></script><script src="/avatars.js" defer></script></head><body><div id="avatars"></div></body></html>' });
      if (url.pathname === '/site.js') return route.fulfill({ contentType: 'text/javascript', body: siteScript });
      if (url.pathname === '/site.css') return route.fulfill({ contentType: 'text/css', body: siteStyles });
      if (url.pathname === '/avatars.js') return route.fulfill({ contentType: 'text/javascript', body: script });
      if (url.pathname === '/avatars.css') return route.fulfill({ contentType: 'text/css', body: stylesheet });
      if (url.pathname === '/auth/status') {
        const body = JSON.stringify({ private_site: false, authenticated: true, member: cookieUser ? { user: cookieUser } : null });
        const gate = pauseStatus;
        if (gate) { pauseStatus = null; gate.started(); await gate.released; }
        return route.fulfill({ contentType: 'application/json', body });
      }
      if (url.pathname === '/api/avatars') {
        const body = JSON.stringify(metadata), gate = pauseMetadata;
        if (gate) { pauseMetadata = null; gate.started(); await gate.released; }
        return route.fulfill({ contentType: 'application/json', body });
      }
      if (url.pathname.startsWith('/api/avatar/')) {
        const user = decodeURIComponent(url.pathname.slice('/api/avatar/'.length));
        if (request.method() === 'GET') {
          imageRequests++;
          return route.fulfill(broken.has(user) || !metadata[user] ? { status: 404, body: 'No picture' } : { contentType: 'image/png', body: image });
        }
        const record = { user, method: request.method(), headers: request.headers(), body: request.postDataBuffer() };
        requests.push(record);
        const authorized = record.headers.authorization
          ? record.headers.authorization === `Bearer qa-${user}-token`
          : cookieUser === user;
        if (!authorized) return route.fulfill({ status: 401, body: 'Unauthorized' });
        if (!record.headers.authorization && record.headers['x-ankiquest-csrf'] !== '1') return route.fulfill({ status: 403, body: 'CSRF required' });
        const gate = pauseWrite;
        if (gate) { pauseWrite = null; gate.started(); await gate.released; }
        if (request.method() === 'DELETE') {
          delete metadata[user];
          return route.fulfill({ status: 204 }).catch(() => {});
        }
        metadata[user] = String(Number(metadata[user] || 0) + 1);
        return route.fulfill({ contentType: 'application/json', body: JSON.stringify({ revision: metadata[user] }) }).catch(() => {});
      }
      return route.abort();
    });
    await page.goto('https://avatar.test/');
    await page.waitForFunction(() => !!window.AnkiQuestAvatars);
    await page.evaluate(() => AnkiQuestAvatars.refresh());
    async function picture(width, height) {
      const base64 = await page.evaluate(({ width, height }) => {
        const canvas = document.createElement('canvas'); canvas.width = width; canvas.height = height;
        const ctx = canvas.getContext('2d');
        ctx.fillStyle = '#ff0000'; ctx.fillRect(0, 0, width, height);
        ctx.fillStyle = '#00ff00';
        if (width > height) ctx.fillRect((width - height) / 2, 0, height, height);
        else ctx.fillRect(0, (height - width) / 2, width, width);
        return canvas.toDataURL('image/png').split(',')[1];
      }, { width, height });
      return Buffer.from(base64, 'base64');
    }
    image = Buffer.from(await page.evaluate(() => {
      const canvas = document.createElement('canvas'); canvas.width = canvas.height = 20;
      return canvas.toDataURL('image/png').split(',')[1];
    }), 'base64'); // Fully transparent: initials must not show through a loaded picture.
    const landscape = await picture(600, 200), portrait = await picture(200, 600);
    const file = buffer => ({ name: 'synthetic.png', mimeType: 'image/png', buffer });
    const dialog = page.locator('dialog.avatar-editor');
    const save = dialog.locator('button[type=submit]');
    const status = text => dialog.locator('.avatar-status').filter({ hasText: text }).waitFor();
    const choose = async buffer => { await dialog.locator('[name=picture]').setInputFiles(file(buffer)); await status('Ready to save.'); };
    const open = async (user = 'cerro', token = '') => { await page.evaluate(({ user, token }) => AnkiQuestAvatars.open({ user, display: user, token }), { user, token }); await dialog.waitFor(); };
    const close = async () => { await dialog.locator('[data-close]').click(); await dialog.waitFor({ state: 'detached' }); };
    const refresh = () => page.evaluate(() => AnkiQuestAvatars.refresh());
    function gate() {
      let started, release;
      return { started: () => started(), reached: new Promise(resolve => { started = resolve; }), released: new Promise(resolve => { release = resolve; }), release: () => release() };
    }
    async function assertNormalized(buffer) {
      assert.deepEqual([...buffer.subarray(0, 8)], [137, 80, 78, 71, 13, 10, 26, 10], 'Raw PNG body, not multipart or JSON');
      assert.equal(buffer.readUInt32BE(16), 256); assert.equal(buffer.readUInt32BE(20), 256);
      const pixels = await page.evaluate(async base64 => {
        const blob = await fetch('data:image/png;base64,' + base64).then(response => response.blob());
        const bitmap = await createImageBitmap(blob), canvas = document.createElement('canvas');
        canvas.width = canvas.height = 256; const ctx = canvas.getContext('2d'); ctx.drawImage(bitmap, 0, 0); bitmap.close();
        return [[8, 8], [247, 8], [8, 247], [247, 247], [128, 128]].map(([x, y]) => [...ctx.getImageData(x, y, 1, 1).data]);
      }, buffer.toString('base64'));
      for (const pixel of pixels) assert.deepEqual(pixel, [0, 255, 0, 255], 'Center crop preserves aspect ratio and excludes the red outer bands');
    }

    const escapedUser = 'qa-"/<player>?&', escapedName = '<img src=x> Alice';
    await page.evaluate(({ escapedUser, escapedName }) => {
      document.getElementById('avatars').innerHTML = AnkiQuestSite.avatar('cerro', 'Cerro') + AnkiQuestSite.avatar('missing', 'Missing Picture') + AnkiQuestSite.avatar('broken', 'Broken Picture') + AnkiQuestSite.avatar(escapedUser, escapedName);
    }, { escapedUser, escapedName });
    assert.equal(await page.locator('#avatars img').count(), 0, 'Absent metadata uses initials without image requests');
    assert.equal(await page.locator('#avatars .avatar').last().getAttribute('data-avatar-user'), escapedUser);
    assert.equal(await page.locator('#avatars .avatar').last().textContent(), '<S', 'Display text is escaped');
    metadata.cerro = '1'; metadata.broken = '1'; metadata[escapedUser] = '1'; metadata.missing = 'https://invalid.example/picture';
    broken.add('broken');
    await refresh();
    const cerro = page.locator('#avatars [data-avatar-user=cerro]');
    await cerro.locator('.avatar-photo[data-loaded]').waitFor();
    assert(!(await cerro.locator('.avatar-initials').isVisible()), 'Loaded transparent picture hides the initials underneath');
    await page.waitForFunction(() => !document.querySelector('#avatars [data-avatar-user=broken] img'));
    assert.equal(await page.locator('#avatars [data-avatar-user=missing] img').count(), 0, 'Invalid revisions are ignored');
    assert.equal(await page.locator('#avatars [data-avatar-user=broken]').textContent(), 'BP', 'Broken image keeps initials');
    assert(await page.locator('#avatars [data-avatar-user=broken] .avatar-initials').isVisible(), 'Broken image restores visible initials');
    await page.locator('#avatars .avatar').last().locator('.avatar-photo[data-loaded]').waitFor();
    assert.equal(await page.locator('#avatars .avatar').last().locator('img').getAttribute('src'), '/api/avatar/' + encodeURIComponent(escapedUser) + '?v=1');
    const previousRequests = imageRequests;
    await page.evaluate(async () => { document.body.append(document.createElement('div')); await new Promise(requestAnimationFrame); await new Promise(requestAnimationFrame); });
    assert.equal(imageRequests, previousRequests, 'Broken image does not retry in an observer loop');
    checks.push('metadata rendering, missing/broken fallback, escaped names/users, safe revision URLs');

    metadata.cerro = '2'; await refresh(); await cerro.locator('img[data-revision="2"][data-loaded]').waitFor();
    delete metadata.cerro; await refresh(); assert.equal(await cerro.locator('img').count(), 0);
    assert(await cerro.locator('.avatar-initials').isVisible(), 'Removing a picture restores visible initials');
    assert.equal(await cerro.textContent(), 'C');
    metadata.cerro = '3'; await refresh(); await cerro.locator('img[data-revision="3"][data-loaded]').waitFor();
    checks.push('external replacement and removal refresh all existing avatars');

    await open('cerro', 'qa-cerro-token');
    await page.evaluate(() => dispatchEvent(new Event('ankiquest-auth')));
    assert.equal(await page.evaluate(() => AnkiQuestAvatars.isOpen()), true, 'Repeating the native identity supplied before script loading preserves the editor');
    await close();
    await page.evaluate(() => { window.ankiquestSession = null; dispatchEvent(new Event('ankiquest-auth')); });

    await open(); assert(await save.isDisabled());
    await dialog.locator('[data-remove]').click(); await status('Enter your AnkiQuest token'); assert.equal(requests.length, 0);
    await choose(landscape); await save.click(); assert.equal(requests.length, 0, 'Required password blocks unauthenticated upload');
    await dialog.locator('[name=token]').fill('wrong-token'); await save.click(); await status('That token was not accepted');
    assert.equal(requests.length, 1); assert(!(await save.isDisabled()), 'Auth failure permits retry without losing selected picture');
    await dialog.locator('[name=token]').fill('qa-cerro-token'); await save.click(); await status('Profile picture saved.');
    const firstUpload = requests.at(-1);
    assert.equal(firstUpload.method, 'POST'); assert.equal(firstUpload.headers.authorization, 'Bearer qa-cerro-token'); assert.equal(firstUpload.headers['content-type'], 'image/png');
    await assertNormalized(firstUpload.body);
    await cerro.locator('img[data-revision="4"][data-loaded]').waitFor();
    assert(await save.isDisabled()); assert.equal(await page.evaluate(() => activePreviewURLs.size), 0);
    await close();
    checks.push('missing/invalid auth, retry, raw authenticated PNG upload and landscape center crop');

    await open('cerro', 'qa-cerro-token'); assert.equal(await dialog.locator('[name=token]').count(), 0);
    await choose(portrait); await save.click(); await status('Profile picture saved.'); await assertNormalized(requests.at(-1).body);
    await dialog.locator('[data-remove]').click(); await status('Picture removed.');
    assert.equal(requests.at(-1).method, 'DELETE'); assert.equal(requests.at(-1).headers.authorization, 'Bearer qa-cerro-token');
    assert.equal(await cerro.locator('img').count(), 0); assert(await dialog.locator('[data-remove]').isDisabled());
    await close();
    checks.push('supplied-token editor, portrait center crop, authenticated deletion and immediate initials fallback');

    cookieUser = 'cerro';
    await page.evaluate(() => AnkiQuestSite.status(true, false));
    await open(); await choose(landscape);
    assert.equal(await dialog.locator('[name=token]').count(), 0, 'An existing matching owner session needs no token prompt');
    await save.click(); await status('Profile picture saved.');
    assert.equal(requests.at(-1).headers.authorization, undefined);
    assert.equal(requests.at(-1).headers['x-ankiquest-csrf'], '1');
    assert.equal(requests.at(-1).headers['content-type'], 'image/png');
    await assertNormalized(requests.at(-1).body);
    await dialog.locator('[data-remove]').click(); await status('Picture removed.');
    assert.equal(requests.at(-1).method, 'DELETE');
    assert.equal(requests.at(-1).headers.authorization, undefined);
    assert.equal(requests.at(-1).headers['x-ankiquest-csrf'], '1');
    await close();
    checks.push('matching cookie owner saves raw PNG and removes photos with CSRF, no token prompt or Bearer header');

    cookieUser = 'alice';
    await page.evaluate(() => AnkiQuestSite.status(true, false));
    await open(); await choose(portrait);
    await dialog.locator('[name=token]').fill('qa-cerro-token');
    await save.click(); await status('Profile picture saved.');
    assert.equal(requests.at(-1).user, 'cerro');
    assert.equal(requests.at(-1).headers.authorization, 'Bearer qa-cerro-token');
    await close();
    checks.push('a different member cookie does not authorize the selected profile; manual token fallback stays scoped');

    cookieUser = 'cerro';
    await page.evaluate(() => AnkiQuestSite.status(true, false));
    await open('cerro', 'incorrect-explicit-token'); await choose(landscape);
    await save.click(); await status('That token was not accepted');
    assert.equal(requests.at(-1).headers.authorization, 'Bearer incorrect-explicit-token', 'An explicit credential takes precedence over a valid cookie');
    await dialog.locator('[name=token]').fill('qa-cerro-token');
    await save.click(); await status('Profile picture saved.');
    await close();
    checks.push('explicit Bearer retains priority and a rejected supplied token can be corrected without reopening');

    await open(); await choose(landscape);
    cookieUser = null;
    await save.click(); await status('Your member session expired or changed');
    assert.equal(requests.at(-1).headers.authorization, undefined);
    await dialog.locator('[name=token]').fill('qa-cerro-token');
    await save.click(); await status('Profile picture saved.');
    await assertNormalized(requests.at(-1).body);
    await close();
    checks.push('expired owner session exposes manual recovery and preserves the selected photo for retry');

    cookieUser = 'cerro';
    const delayedIdentity = gate(); pauseStatus = delayedIdentity;
    await page.evaluate(() => { window.pendingAvatarStatus = AnkiQuestSite.status(true, false); });
    await delayedIdentity.reached;
    await open();
    assert(await save.isDisabled(), 'No mutation while member ownership is unresolved');
    await page.evaluate(() => dispatchEvent(new CustomEvent('ankiquest:identity', { detail: { user: 'alice' } })));
    assert.equal(await dialog.count(), 0);
    await open('alice', 'qa-alice-token');
    delayedIdentity.release(); await page.evaluate(() => pendingAvatarStatus);
    assert.equal(await dialog.locator('.avatar-preview .avatar').getAttribute('data-avatar-user'), 'alice');
    assert.equal(await dialog.locator('[name=token]').count(), 0);
    await close();
    cookieUser = null;
    await page.evaluate(() => AnkiQuestSite.status(true, false));
    checks.push('identity change during owner lookup closes the old editor and discards its late response');

    await page.evaluate(() => {
      window.ankiquestSession = { user: 'cerro', token: '  qa-cerro-token  ' };
      dispatchEvent(new Event('ankiquest-auth'));
    });
    await open(); await choose(landscape);
    assert.equal(await dialog.locator('[name=token]').count(), 0, 'The profile entry point reuses a matching native token without being passed one');
    await save.click(); await status('Profile picture saved.');
    assert.equal(requests.at(-1).headers.authorization, 'Bearer qa-cerro-token', 'Native token is trimmed');
    await dialog.locator('[data-remove]').click(); await status('Picture removed.');
    assert.equal(requests.at(-1).headers.authorization, 'Bearer qa-cerro-token');
    await close();
    await open('cerro', 'explicit-invalid'); await choose(landscape); await save.click(); await status('That token was not accepted');
    assert.equal(requests.at(-1).headers.authorization, 'Bearer explicit-invalid', 'Explicit token precedes even a matching native token');
    await close();
    checks.push('matching native token is reused for save and remove; explicit credentials retain precedence');

    cookieUser = 'cerro';
    await page.evaluate(() => {
      window.ankiquestSession = { user: 'alice', token: 'qa-alice-token' };
      dispatchEvent(new Event('ankiquest-auth'));
      return AnkiQuestSite.status(true, false);
    });
    await open(); await choose(landscape);
    const beforeMismatch = requests.length;
    await save.click();
    assert.equal(requests.length, beforeMismatch, 'A different native identity blocks automatic use of the old matching owner cookie');
    await dialog.locator('[name=token]').fill('qa-cerro-token'); await save.click(); await status('Profile picture saved.');
    assert.equal(requests.at(-1).user, 'cerro');
    assert.equal(requests.at(-1).headers.authorization, 'Bearer qa-cerro-token');
    await close();
    await page.evaluate(() => {
      window.ankiquestSession = { user: 'cerro', token: 123 };
      dispatchEvent(new Event('ankiquest-auth'));
    });
    await open(); await choose(landscape); await save.click(); await status('Profile picture saved.');
    assert.equal(requests.at(-1).headers.authorization, undefined, 'Malformed native credentials are not adopted as a token');
    await close();
    cookieUser = null;
    await page.evaluate(() => {
      window.ankiquestSession = { user: 'cerro', token: 'initial-native-token' };
      dispatchEvent(new Event('ankiquest-auth'));
      return AnkiQuestSite.status(true, false);
    });
    checks.push('mismatched native account requires manual target authentication; malformed native token cannot become a credential');

    await open('cerro', 'qa-cerro-token');
    await dialog.locator('[name=picture]').setInputFiles({ name: 'invalid.svg', mimeType: 'image/svg+xml', buffer: Buffer.from('<svg xmlns="http://www.w3.org/2000/svg"/>') });
    await status('Choose a JPEG or PNG'); assert(await save.isDisabled());
    await dialog.locator('[name=picture]').setInputFiles({ name: 'broken.png', mimeType: 'image/png', buffer: Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]) });
    await status('Could not read this picture'); assert(await save.isDisabled());
    await close(); checks.push('unsupported and undecodable file feedback');

    await page.evaluate(() => {
      window.realCreateImageBitmap = createImageBitmap; window.decodeGates = [];
      window.createImageBitmap = (...args) => new Promise((resolve, reject) => decodeGates.push(() => realCreateImageBitmap(...args).then(resolve, reject)));
    });
    await open('cerro', 'qa-cerro-token');
    await dialog.locator('[name=picture]').setInputFiles(file(landscape)); await page.waitForFunction(() => decodeGates.length === 1);
    await dialog.locator('[name=picture]').setInputFiles(file(portrait)); await page.waitForFunction(() => decodeGates.length === 2);
    await page.evaluate(() => decodeGates[1]()); await status('Ready to save.');
    const newerPreview = await dialog.locator('.avatar-preview>img').getAttribute('src');
    await page.evaluate(() => decodeGates[0]());
    assert.equal(await dialog.locator('.avatar-preview>img').getAttribute('src'), newerPreview, 'Stale decode cannot replace newer selection');
    await close();
    await open('cerro', 'qa-cerro-token');
    await dialog.locator('[name=picture]').setInputFiles(file(landscape)); await page.waitForFunction(() => decodeGates.length === 3);
    await close(); await open('alice', 'qa-alice-token'); await page.evaluate(() => decodeGates[2]());
    assert.equal(await dialog.locator('.avatar-preview>img').count(), 0, 'Closed editor decode cannot enter another account');
    assert.equal(await dialog.locator('.avatar-preview .avatar').getAttribute('data-avatar-user'), 'alice');
    await close(); await page.evaluate(() => { window.createImageBitmap = realCreateImageBitmap; });
    assert.equal(await page.evaluate(() => activePreviewURLs.size), 0);
    checks.push('out-of-order selection and close-during-decode account guards; preview URL cleanup');

    await open('cerro', 'qa-cerro-token'); await choose(landscape);
    const write = gate(); pauseWrite = write; await save.click(); await write.reached;
    await close(); await open('alice', 'qa-alice-token'); write.release();
    await page.evaluate(async () => { await new Promise(requestAnimationFrame); await new Promise(requestAnimationFrame); });
    assert.equal(await dialog.locator('.avatar-preview .avatar').getAttribute('data-avatar-user'), 'alice');
    assert(!(await dialog.locator('.avatar-status').textContent()).includes('saved'), 'Closed upload cannot update a new editor');
    await close(); checks.push('close-during-upload does not mutate another account editor');

    await refresh(); await open('cerro', 'qa-cerro-token'); await choose(landscape); await refresh();
    const staleMetadata = gate(); pauseMetadata = staleMetadata;
    await page.evaluate(() => { window.heldRefresh = AnkiQuestAvatars.refresh(); }); await staleMetadata.reached;
    await save.click(); await status('Profile picture saved.'); const savedRevision = metadata.cerro;
    staleMetadata.release(); await page.evaluate(() => heldRefresh);
    await cerro.locator(`img[data-revision="${savedRevision}"][data-loaded]`).waitFor();
    await close(); checks.push('stale metadata response cannot undo a saved revision');

    await open('cerro', 'qa-cerro-token'); await choose(landscape);
    await page.evaluate(() => dispatchEvent(new CustomEvent('ankiquest:identity', { detail: { user: 'alice' } })));
    assert.equal(await page.evaluate(() => AnkiQuestAvatars.isOpen()), false, 'Changing member identity closes the previous account editor');
    assert.equal(await page.evaluate(() => activePreviewURLs.size), 0, 'Changing member identity releases the selected picture');
    checks.push('member account replacement clears editor credentials and selected picture');

    await open('cerro', 'qa-cerro-token'); await choose(landscape);
    await page.evaluate(() => dispatchEvent(new Event('ankiquest-auth')));
    assert.equal(await page.evaluate(() => AnkiQuestAvatars.isOpen()), true, 'Repeating the current native identity preserves the draft');
    await page.evaluate(() => { window.ankiquestSession = { user: 'alice', token: 'replacement' }; dispatchEvent(new Event('ankiquest-auth')); });
    assert.equal(await page.evaluate(() => AnkiQuestAvatars.isOpen()), false, 'Replacing the native account closes the previous account editor');
    assert.equal(await page.evaluate(() => activePreviewURLs.size), 0, 'Replacing the native account releases the selected picture');
    checks.push('native account replacement clears editor credentials and selected picture');
    await open('alice', 'qa-alice-token'); await choose(landscape);
    await page.evaluate(() => dispatchEvent(new Event('ankiquest-auth')));
    assert.equal(await page.evaluate(() => AnkiQuestAvatars.isOpen()), true, 'Repeated identical native session preserves the editor draft');
    await page.evaluate(() => { window.ankiquestSession = null; dispatchEvent(new Event('ankiquest-auth')); });
    assert.equal(await page.evaluate(() => AnkiQuestAvatars.isOpen()), false, 'Removing the native account clears the editor');
    assert.equal(await page.evaluate(() => activePreviewURLs.size), 0);

    await open('cerro', 'qa-cerro-token'); await choose(landscape);
    await page.evaluate(() => { window.ankiquestSession = null; dispatchEvent(new CustomEvent('ankiquest-auth')); });
    assert.equal(await page.evaluate(() => AnkiQuestAvatars.isOpen()), false, 'An explicit empty native account also clears an editor opened without a native identity');
    assert.equal(await page.evaluate(() => activePreviewURLs.size), 0, 'Explicit empty native account releases the selected picture');
    checks.push('explicit empty native identity clears existing manual or cookie-account editor');

    await open('cerro', 'qa-cerro-token'); await choose(landscape); await refresh();
    const lockedMetadata = gate(); pauseMetadata = lockedMetadata;
    await page.evaluate(() => { window.lockedRefresh = AnkiQuestAvatars.refresh(); }); await lockedMetadata.reached;
    await page.evaluate(() => dispatchEvent(new Event('ankiquest:locked')));
    assert.equal(await page.evaluate(() => AnkiQuestAvatars.isOpen()), false, 'Locking the site closes the authenticated picture editor');
    assert.equal(await page.locator('.avatar-photo').count(), 0, 'Locking the site clears displayed pictures');
    lockedMetadata.release(); await page.evaluate(() => lockedRefresh);
    await page.evaluate(() => {
      document.getElementById('avatars').insertAdjacentHTML('beforeend', AnkiQuestAvatars.markup('cerro', 'Cerro'));
      return AnkiQuestAvatars.refresh();
    });
    assert.equal(await page.locator('.avatar-photo').count(), 0, 'Late metadata and later refreshes cannot restore pictures after locking');
    checks.push('site lock clears editor, tokens, preview URLs and photo memory; late responses stay cleared');

    const privateState = await page.evaluate(() => ({ local: JSON.stringify(localStorage), session: JSON.stringify(sessionStorage), html: document.body.innerHTML, open: AnkiQuestAvatars.isOpen(), previews: activePreviewURLs.size }));
    for (const value of [privateState.local, privateState.session, privateState.html]) assert(!value.includes('qa-cerro-token') && !value.includes('qa-alice-token'), 'Tokens do not survive in page or storage');
    assert.equal(privateState.open, false); assert.equal(privateState.previews, 0); assert.deepEqual(errors, []);
    checks.push('closed editor clears tokens and previews; no page errors');
    console.log(`PASS: ${checks.length} photo behavior groups\n${checks.map(check => '- ' + check).join('\n')}`);
  } finally { await browser.close(); }
}
main().catch(error => { console.error(error.stack); process.exitCode = 1; });
