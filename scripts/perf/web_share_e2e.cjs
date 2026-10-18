// Browser + Rust synthetic source + local Cloudflare worker. No Discord login needed.
// First: cargo test --release --lib screen_capture::web_share::tests --no-run
// Start Wrangler locally on 8787, then: node scripts/perf/web_share_e2e.cjs <test-exe>
const fs = require('node:fs');
const path = require('node:path');
const {spawn, execFile} = require('node:child_process');
const {promisify} = require('node:util');
const exec = promisify(execFile);
const assert = require('node:assert/strict');
const root = path.resolve(__dirname, '../..');
const artifact = path.join(root, 'target/web-share-e2e');
fs.mkdirSync(artifact, {recursive: true});
const ready = path.join(artifact, `ready-${Date.now()}.json`);
const manual = process.argv.includes('--manual');
const exchange = ready + '.offer';
function playwright() {
  function load(entry) {
    const core = require.resolve('playwright-core/package.json', {paths: [path.dirname(entry)]});
    return {browser: require(entry), TestWebSocket: require(path.join(path.dirname(core), 'lib/utilsBundle.js')).ws};
  }
  try { return load(require.resolve('playwright')); } catch {}
  const cache = path.join(process.env.LOCALAPPDATA, 'npm-cache/_npx');
  for (const dir of fs.readdirSync(cache)) {
    const pkg = path.join(cache, dir, 'node_modules/playwright');
    if (fs.existsSync(path.join(pkg, 'package.json'))) return load(require.resolve(pkg));
  }
  throw new Error('Install playwright, or provide it in node_modules');
}
const sleep = ms => new Promise(r => setTimeout(r, ms));
const drivers = playwright();
async function waitReady() {
  for (let i = 0; i < 300; i++) {
    if (fs.existsSync(ready)) return JSON.parse(fs.readFileSync(ready, 'utf8'));
    await sleep(100);
  }
  throw new Error('Source did not start; inspect fixture.log');
}
async function stats(page) {
  return page.evaluate(async () => {
    const records = [];
    for (const pc of window.__peers || []) {
      for (const entry of (await pc.getStats()).values()) {
        if (entry.type === 'inbound-rtp') records.push({
          kind: entry.kind, bytes: entry.bytesReceived, frames: entry.framesDecoded,
          samples: entry.totalSamplesReceived, energy: entry.totalAudioEnergy,
          codec: entry.codecId,
        });
      }
    }
    return {state: document.querySelector('#status').dataset.state, records};
  });
}

async function nativeSignalingRegression() {
  const peers = [];
  function peer(user, channel, version = 2) {
    const ws = new drivers.TestWebSocket(`ws://127.0.0.1:8787/ws?user=${user}&channel=${channel}&v=${version}`);
    const messages = [];
    ws.onmessage = event => messages.push(event.data);
    const item = {ws, messages, async wait(predicate) {
      for (let i = 0; i < 100; i++) {
        const value = messages.find(predicate); if (value) return value;
        await sleep(50);
      }
      throw new Error('Native signaling regression: missing expected message');
    }};
    peers.push(item); return item;
  }
  try {
    const room = 'regression-' + Date.now();
    const a = peer('101', room); await a.wait(s => JSON.parse(s).op === 'room');
    const b = peer('202', room); await b.wait(s => JSON.parse(s).op === 'room');
    await a.wait(s => JSON.parse(s).op === 'peer_join');
    const c = peer('303', room + '-isolated'); await c.wait(s => JSON.parse(s).op === 'room');
    const payload = JSON.stringify({op: 'test-presence', uid: '101'});
    a.ws.send(payload); await b.wait(s => s === payload);
    await sleep(100);
    assert(!a.messages.includes(payload), 'Native relay must not echo');
    assert(!c.messages.includes(payload), 'Native rooms must remain isolated');
    a.ws.send('{"op":"ping"}'); await a.wait(s => JSON.parse(s).op === 'pong');
    a.ws.close(); await b.wait(s => JSON.parse(s).op === 'peer_leave');
    const old = peer('404', room, 1); await old.wait(s => JSON.parse(s).op === 'upgrade_required');
    console.log('PASS: native signaling room/join/relay/no-echo/isolation/ping/leave/old-version regression');
  } finally { for (const p of peers) p.ws.terminate(); }
}

(async () => {
  assert(process.argv[2], 'Pass the Rust test executable as the first argument');
  const source = spawn(path.resolve(process.argv[2]), [
    '--exact', 'screen_capture::web_share::tests::browser_fixture', '--ignored', '--nocapture',
  ], {cwd: root, env: {...process.env, LITECORD_WEB_SIGNALING_URL: 'ws://127.0.0.1:8787',
    // Deliberately unreachable: web sharing must use its independent endpoint.
    LITECORD_SIGNALING_URL: 'ws://127.0.0.1:1',
    ...(manual ? {LITECORD_WEB_MANUAL: '1', LITECORD_WEB_TEST_EXCHANGE: exchange} : {LITECORD_WEB_MANUAL: '0'}),
    LITECORD_WEB_TEST_READY: ready, RUST_LOG: 'info'}});
  const log = fs.createWriteStream(path.join(artifact, 'fixture.log'));
  source.stdout.pipe(log); source.stderr.pipe(log);
  let exited = new Promise(resolve => source.once('exit', code => resolve(code)));
  let browser;
  const results = {};
  async function sample() {
    const result = await exec('python', [path.join(__dirname, 'process_sample.py'), String(source.pid), '3']);
    return JSON.parse(result.stdout);
  }
  try {
    if (!manual) await nativeSignalingRegression();
    const {url} = await waitReady();
    results.source_without_viewers = await sample();
    browser = await drivers.browser.chromium.launch({
      executablePath: 'C:/Program Files/Google/Chrome/Application/chrome.exe',
      headless: true, args: ['--autoplay-policy=no-user-gesture-required'],
    });
    const context = await browser.newContext({locale: 'pt-BR'});
    await context.addInitScript(() => {
      window.__peers = [];
      const Real = window.RTCPeerConnection;
      window.RTCPeerConnection = class extends Real {
        constructor(...args) {
          super(...args); window.__peers.push(this);
          this.addEventListener('connectionstatechange', () => console.log('RTC', performance.now(), this.connectionState));
          this.addEventListener('iceconnectionstatechange', () => console.log('ICE', performance.now(), this.iceConnectionState));
        }
      };
      document.addEventListener('DOMContentLoaded', () => {
        const status = document.querySelector('#status');
        if (status) new MutationObserver(() => console.log('STATUS', performance.now(), status.dataset.state))
          .observe(status, {attributes: true, childList: true});
      });
    });
    async function open(link) {
      const page = await context.newPage();
      page.on('console', msg => fs.appendFileSync(path.join(artifact, 'browser.log'), msg.text() + '\n'));
      page.on('pageerror', error => fs.appendFileSync(path.join(artifact, 'browser.log'), String(error) + '\n'));
      await page.goto(link);
      if (manual) {
        await page.waitForFunction(() => document.querySelector('#manual-offer').value.length > 0);
        const offer = JSON.parse(await page.locator('#manual-offer').inputValue());
        const id = String(Date.now());
        fs.writeFileSync(exchange, JSON.stringify({id, offer}));
        let response;
        for (let i = 0; i < 200; i++) {
          try { response = JSON.parse(fs.readFileSync(exchange + '.answer', 'utf8')); } catch {}
          if (response?.id === id) break;
          await sleep(50);
        }
        assert(response?.answer, response?.error || 'Manual answer timed out');
        await page.locator('#manual-answer').fill(JSON.stringify(response.answer));
        await page.locator('#manual-connect').click();
      }
      return page;
    }
    const a = await open(url);
    await a.waitForFunction(() => document.querySelector('#status').dataset.state === 'playing', {timeout: 35000});
    await a.locator('#play').click();
    await sleep(2000);
    results.first = await stats(a);
    assert.equal(results.first.state, 'playing', 'Playback must remain active after enabling sound');
    results.source_with_one_viewer = await sample();
    fs.writeFileSync(path.join(artifact, 'results.json'), JSON.stringify(results, null, 2));
    assert(results.first.records.some(r => r.kind === 'video' && r.frames > 5), 'H.264 must decode');
    assert(results.first.records.some(r => r.kind === 'audio' && r.samples > 9600 && r.energy > 0), 'Opus must decode non-silent audio');
    await a.screenshot({path: path.join(artifact, 'watch.png')});
    console.log('PASS: Chrome decoded H.264 video and non-silent Opus audio');
    if (manual) {
      await sleep(25000);
      results.steady = await stats(a);
      assert.equal(results.steady.state, 'playing', 'Manual playback must survive the connection timeout');
      const initialFrames = results.first.records.find(r => r.kind === 'video').frames;
      assert(results.steady.records.some(r => r.kind === 'video' && r.frames > initialFrames + 150), 'Manual video must keep decoding');
      fs.writeFileSync(ready + '.stop', 'stop');
      assert.equal(await exited, 0);
      console.log('PASS: manual offer/answer exchange works without a signaling worker');
      fs.writeFileSync(path.join(artifact, 'manual-results.json'), JSON.stringify(results, null, 2));
      return;
    }

    const bad = new URL(url); const hash = new URLSearchParams(bad.hash.slice(1));
    hash.set('key', '00'.repeat(32)); bad.hash = hash.toString();
    const wrong = await open(bad.href);
    await wrong.waitForFunction(() => document.querySelector('#status').dataset.state === 'invalid', {timeout: 15000});
    await wrong.close();
    console.log('PASS: wrong key refused before media connection');
    await sleep(300);
    const b = await open(url), c = await open(url);
    for (const page of [b, c]) await page.waitForFunction(() => document.querySelector('#status').dataset.state === 'playing', {timeout: 35000});
    const fourth = await open(url);
    await fourth.waitForFunction(() => document.querySelector('#status').dataset.state === 'full', {timeout: 10000});
    await fourth.close();
    console.log('PASS: 3 simultaneous viewers; fourth viewer sees full');
    await c.close(); await sleep(500);
    const replacement = await open(url);
    await replacement.waitForFunction(() => document.querySelector('#status').dataset.state === 'playing', {timeout: 35000});
    results.three = await Promise.all([a, b, replacement].map(stats));
    results.source_with_three_viewers = await sample();
    console.log('PASS: closing a viewer frees its slot');
    fs.writeFileSync(ready + '.stop', 'stop');
    for (const page of [a, b, replacement]) await page.waitForFunction(() => document.querySelector('#status').dataset.state === 'ended', {timeout: 10000});
    assert.equal(await exited, 0, 'Source must shut down cleanly');
    console.log('PASS: stopping the host closes every viewer and releases the session');
    const old = await open(url);
    await old.waitForFunction(() => document.querySelector('#status').dataset.state === 'ended', {timeout: 10000});
    const invalid = await open('http://127.0.0.1:8787/watch#room=invalid');
    await invalid.waitForFunction(() => document.querySelector('#status').dataset.state === 'invalid', {timeout: 5000});
    console.log('PASS: ended and malformed links show clear messages');
    fs.writeFileSync(path.join(artifact, 'results.json'), JSON.stringify(results, null, 2));
  } finally {
    fs.writeFileSync(ready + '.stop', 'stop');
    await browser?.close();
    const code = await Promise.race([exited, sleep(5000).then(() => 'timeout')]);
    if (code === 'timeout') source.kill();
    log.end();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
