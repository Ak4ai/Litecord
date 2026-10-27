// Long web-sharing run on the real desktop, to catch the picture freezing now and then.
// The sender captures the whole screen (1080p60) while an animation runs in part of it;
// one Chrome viewer records, every 5 s, frames decoded and the freezes Chrome itself
// counts. Phases: Litecord window open, in the tray, open again.
// Start the local worker first (cloudflare: wrangler dev --port 8787) and use a release
// build with the automation feature.
//
//   node scripts/perf/web_share_soak.cjs [--minutes=4] [--account=Marido]
const fs = require('node:fs');
const path = require('node:path');
const net = require('node:net');
const {spawn} = require('node:child_process');
const assert = require('node:assert/strict');
const browsers = require('./web_share_browser.cjs');
const root = path.resolve(__dirname, '../..');
const option = (name, fallback) => process.argv.find(v => v.startsWith(`--${name}=`))?.split('=').slice(1).join('=') ?? fallback;
const port = Number(option('port', '39111'));
const minutes = Number(option('minutes', '4'));
const account = option('account', 'Marido');
assert(['Marido', 'Henrique'].includes(account), 'Use an authorized saved test account');
const artifact = path.join(root, 'target/web-share-soak', new Date().toISOString().replace(/[:.]/g, '-'));
const work = path.join(artifact, 'sender');
fs.mkdirSync(work, {recursive: true});
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const samples = [];
const save = () => fs.writeFileSync(path.join(artifact, 'samples.json'), JSON.stringify(samples, null, 1));

function command(value) {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection({host: '127.0.0.1', port});
    let input = '';
    socket.setTimeout(15000, () => socket.destroy(new Error('Automation response timed out')));
    socket.on('error', reject);
    socket.on('connect', () => socket.write(JSON.stringify(value) + '\n'));
    socket.on('data', chunk => {
      input += chunk;
      if (input.includes('\n')) { socket.end(); try { resolve(JSON.parse(input.split('\n')[0])); } catch (e) { reject(e); } }
    });
  });
}
async function waitFor(predicate, label, timeout = 60000) {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    try { const value = await predicate(); if (value) return value; } catch (error) {
      if (!['ECONNREFUSED', 'ECONNRESET'].includes(error.code)) throw error;
    }
    await sleep(500);
  }
  throw new Error(`Timed out: ${label}`);
}
async function invoke(value) {
  const response = await command(value);
  assert(response.ok, `${value.cmd}: ${response.error}`);
  return response;
}
// What Chrome itself reports for the received video.
async function videoStats(page) {
  return page.evaluate(async () => {
    for (const pc of window.__peers || []) {
      const all = await pc.getStats();
      for (const e of all.values()) {
        if (e.type === 'inbound-rtp' && (e.kind ?? e.mediaType) === 'video') {
          return {decoded: e.framesDecoded, dropped: e.framesDropped, received: e.framesReceived,
            freezes: e.freezeCount, freeze_s: e.totalFreezesDuration, pauses: e.pauseCount, pause_s: e.totalPausesDuration,
            lost: e.packetsLost, nacks: e.nackCount, plis: e.pliCount, firs: e.firCount,
            jitter_ms: e.jitterBufferEmittedCount ? 1000 * e.jitterBufferDelay / e.jitterBufferEmittedCount : null,
            keyframes: e.keyFramesDecoded, fps: e.framesPerSecond};
        }
      }
    }
    return null;
  });
}

(async () => {
  console.log(`Artifacts: ${artifact}`);
  const source = spawn(path.join(root, 'target/release/litecord.exe'), [], {
    // Nobody reads the console output here: a pipe left full would block the app.
    cwd: work, windowsHide: true, stdio: 'ignore', env: {...process.env,
      PATH: path.join(root, 'target/release') + path.delimiter + process.env.PATH,
      LITECORD_AUTOMATION_PORT: String(port), LITECORD_TEST_ACCOUNT: account,
      LITECORD_TEST_MUTE_OUTPUT: '1', LITECORD_WEB_SIGNALING_URL: 'ws://127.0.0.1:8787', RUST_LOG: 'info'},
  });
  console.log(`sender pid ${source.pid}`);
  let viewerBrowser, desktop;
  try {
    await waitFor(async () => {
      const s = await command({cmd: 'state'});
      return s.is_logged_in && s.user_id !== '0';
    }, 'login', 90000);
    // The server list arrives a moment after login.
    await waitFor(async () => (await command({cmd: 'open_guild', name: 'Ricogames_ e MariaJulia_'})).ok, 'server list', 60000);
    await sleep(4000);
    await invoke({cmd: 'open_channel', name: 'fofocas'});
    await waitFor(async () => {
      const s = await command({cmd: 'state'});
      return s.voice.is_in_voice && !s.voice.is_connecting && s.voice.channel === 'fofocas';
    }, 'voice channel');
    await invoke({cmd: 'start_screen_share', res: '1080p', fps: 60, screen: 0, audio: false});
    await waitFor(async () => (await command({cmd: 'state'})).stream.is_screen_sharing, 'capture');

    // Something moving on screen, in a window that leaves Litecord visible beside it.
    const api = browsers.drivers().browser;
    desktop = await browsers.launch(api, 'chrome', {headless: false,
      args: ['--window-position=0,0', '--window-size=1100,700', '--mute-audio']});
    const content = await (await desktop.newContext({viewport: null})).newPage();
    await content.setContent(`<style>body{margin:0;background:#18202b;overflow:hidden}canvas{width:100vw;height:100vh}</style>
      <canvas width="1100" height="700"></canvas><script>
      const c=document.querySelector('canvas'),x=c.getContext('2d');
      function draw(t){x.fillStyle='#18202b';x.fillRect(0,0,1100,700);
        for(let i=0;i<26;i++){x.fillStyle='hsl('+(i*13)+',65%,50%)';x.fillRect((t*.15+i*53)%1260-160,i*27,160,22)}
        x.fillStyle='white';x.font='32px sans-serif';x.fillText(new Date().toISOString().slice(11,23),40,680);
        requestAnimationFrame(draw)} requestAnimationFrame(draw);</script>`);

    await invoke({cmd: 'web_share_start'});
    const url = await waitFor(async () => {
      const s = (await command({cmd: 'state'})).stream;
      if (s.web_error) throw new Error(s.web_error);
      return s.web_active && s.web_url;
    }, 'web sharing');
    viewerBrowser = await browsers.launch(api, 'chrome');
    const context = await viewerBrowser.newContext();
    await browsers.instrument(context);
    const page = await context.newPage();
    await page.goto(url);
    await page.waitForFunction(() => document.querySelector('#status').dataset.state === 'playing', null, {timeout: 40000});

    async function phase(label, ms) {
      console.log(`${new Date().toISOString()} phase ${label}`);
      const end = Date.now() + ms;
      let last = await videoStats(page);
      while (Date.now() < end) {
        await sleep(5000);
        const now = await videoStats(page);
        const state = await page.evaluate(() => document.querySelector('#status').dataset.state);
        const d = k => (now?.[k] ?? 0) - (last?.[k] ?? 0);
        const sample = {t: new Date().toISOString(), phase: label, state,
          fps: d('decoded') / 5, freezes: d('freezes'), freeze_s: +d('freeze_s').toFixed(2), dropped: d('dropped'),
          lost: d('lost'), nacks: d('nacks'), plis: d('plis'), keyframes: d('keyframes'),
          jitter_ms: now?.jitter_ms && +now.jitter_ms.toFixed(1)};
        samples.push(sample); save();
        if (sample.freezes || sample.fps < 45 || sample.plis) console.log(JSON.stringify(sample));
        last = now;
      }
    }
    await invoke({cmd: 'restore'});
    await phase('window-open', minutes * 60000);
    await invoke({cmd: 'minimize_to_tray'});
    await phase('tray', minutes * 60000);
    await invoke({cmd: 'restore'});
    await phase('window-open-again', minutes * 60000);

    const by = {};
    for (const s of samples) {
      const p = by[s.phase] ??= {seconds: 0, frames: 0, freezes: 0, freeze_s: 0, plis: 0, lost: 0, slow5s: 0};
      p.seconds += 5; p.frames += s.fps * 5; p.freezes += s.freezes; p.freeze_s += s.freeze_s;
      p.plis += s.plis; p.lost += s.lost; if (s.fps < 45) p.slow5s++;
    }
    for (const [k, p] of Object.entries(by)) {
      console.log(`${k}: ${(p.frames / p.seconds).toFixed(1)} fps decoded, ${p.freezes} freezes (${p.freeze_s.toFixed(1)} s), ${p.slow5s} slow 5-s windows, ${p.plis} PLIs, ${p.lost} packets lost`);
    }
  } finally {
    for (const cmd of ['web_share_stop', 'stop_screen_share', 'leave_voice']) { try { await command({cmd}); } catch {} }
    await viewerBrowser?.close(); await desktop?.close();
    if (source.exitCode === null) source.kill();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
