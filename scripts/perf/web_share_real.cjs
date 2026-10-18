// Real Windows desktop capture, measured on the sender process only.
// Start the local worker first. Use a release build with the automation feature.
const fs = require('node:fs');
const path = require('node:path');
const net = require('node:net');
const {spawn, execFile} = require('node:child_process');
const {promisify} = require('node:util');
const assert = require('node:assert/strict');
const browsers = require('./web_share_browser.cjs');
const exec = promisify(execFile);
const root = path.resolve(__dirname, '../..');
const option = (name, fallback) => process.argv.find(v => v.startsWith(`--${name}=`))?.split('=').slice(1).join('=') ?? fallback;
const port = Number(option('port', '39111'));
const seconds = Number(option('seconds', '20'));
const rounds = Number(option('rounds', '2'));
const quietBuilds = option('quiet-builds', '0') === '1';
const account = option('account', 'Marido');
assert(['Marido', 'Henrique'].includes(account), 'Use an authorized saved test account');
assert(Number.isInteger(port) && port > 1024 && port <= 65535);
assert(Number.isInteger(seconds) && seconds >= 5 && Number.isInteger(rounds) && rounds > 0);
const artifact = path.join(root, 'target/web-share-real', new Date().toISOString().replace(/[:.]/g, '-'));
const work = path.join(artifact, 'sender');
fs.mkdirSync(work, {recursive: true});
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const result = {requested: {resolution: '1920x1080', fps: 60, audio: false, seconds, rounds}, phases: [], browsers: []};
const save = () => fs.writeFileSync(path.join(artifact, 'results.json'), JSON.stringify(result, null, 2));

function command(value) {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection({host: '127.0.0.1', port});
    let input = '';
    socket.setTimeout(15000, () => socket.destroy(new Error('Automation response timed out')));
    socket.on('error', reject);
    socket.on('end', () => { if (!input.includes('\n')) reject(new Error('Automation connection ended without a response')); });
    socket.on('connect', () => socket.write(JSON.stringify(value) + '\n'));
    socket.on('data', chunk => {
      input += chunk;
      if (input.includes('\n')) {
        socket.end();
        try { resolve(JSON.parse(input.split('\n')[0])); } catch (error) { reject(error); }
      }
    });
  });
}
async function waitFor(predicate, label, timeout = 45000) {
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
async function builds() {
  const response = await exec('powershell', ['-NoProfile', '-Command',
    "@(Get-CimInstance Win32_Process | Where-Object { $_.Name -in @('rustc.exe','ld.exe','link.exe') } | Select-Object ProcessId,ParentProcessId) | ConvertTo-Json -Compress"], {windowsHide: true});
  const value = response.stdout.trim();
  return value ? [].concat(JSON.parse(value)) : [];
}
async function waitForBuilds() {
  if ((await builds()).length === 0) return;
  console.log('Waiting for active compilers before measuring the sender');
  await waitFor(async () => (await builds()).length === 0, 'compilers to finish', 600000);
}
async function watch(context, url) {
  const page = await context.newPage();
  page.on('pageerror', error => fs.appendFileSync(path.join(artifact, 'browser-errors.log'), error.message + '\n'));
  await page.goto(url);
  await page.waitForFunction(() => document.querySelector('#status').dataset.state === 'playing', null, {timeout: 40000});
  return page;
}

(async () => {
  console.log(`Artifacts: ${artifact}`);
  if (quietBuilds) await waitForBuilds();
  await new Promise((resolve, reject) => {
    const check = net.createServer(); check.on('error', reject);
    check.listen(port, '127.0.0.1', () => check.close(resolve));
  });
  const source = spawn(path.join(root, 'target/release/litecord.exe'), [], {
    cwd: work, windowsHide: true, env: {...process.env,
      PATH: path.join(root, 'target/release') + path.delimiter + process.env.PATH,
      LITECORD_AUTOMATION_PORT: String(port), LITECORD_TEST_ACCOUNT: account,
      LITECORD_TEST_MUTE_OUTPUT: '1', LITECORD_WEB_SIGNALING_URL: 'ws://127.0.0.1:8787',
      LITECORD_WEB_MANUAL: '0', RUST_LOG: 'info'},
  });
  source.on('error', error => { result.launch_error = error.message; save(); });
  const log = fs.createWriteStream(path.join(artifact, 'sender-output.log'));
  source.stdout.pipe(log); source.stderr.pipe(log);
  result.pid = source.pid;
  let chrome, desktop, extra;
  let controlsOwned = false;
  try {
    await waitFor(async () => {
      if (source.exitCode !== null) throw new Error(`Sender exited: ${source.exitCode}`);
      try { return (await command({cmd: 'state'})).ok; } catch (error) {
        if (['ECONNREFUSED', 'ECONNRESET'].includes(error.code)) return false;
        throw error;
      }
    }, 'automation startup', 60000);
    const owner = await exec('powershell', ['-NoProfile', '-Command',
      `(Get-NetTCPConnection -LocalPort ${port} -State Listen).OwningProcess`], {windowsHide: true});
    assert.equal(Number(owner.stdout.trim()), source.pid, 'Only control the sender process started by this test');
    controlsOwned = true;
    await waitFor(async () => {
      if (source.exitCode !== null) throw new Error(`Sender exited: ${source.exitCode}`);
      const state = await command({cmd: 'state'});
      return state.is_logged_in && state.user_id !== '0';
    }, 'saved account login', 90000);
    await invoke({cmd: 'open_guild', name: 'Ricogames_ e MariaJulia_'});
    await sleep(4000);
    await invoke({cmd: 'open_channel', name: 'fofocas'});
    await waitFor(async () => {
      const state = await command({cmd: 'state'});
      return state.voice.is_in_voice && !state.voice.is_connecting && state.voice.channel === 'fofocas';
    }, 'authorized voice channel');
    await invoke({cmd: 'start_screen_share', res: '1080p', fps: 60, screen: 0, audio: false});
    await waitFor(async () => (await command({cmd: 'state'})).stream.is_screen_sharing, 'real screen capture');
    await invoke({cmd: 'minimize_to_tray'});

    const api = browsers.drivers().browser;
    chrome = await browsers.launch(api, 'chrome');
    const context = await chrome.newContext({locale: 'pt-BR'});
    await browsers.instrument(context);
    // The OS captures this actual desktop window; no frames are injected into the sender.
    desktop = await browsers.launch(api, 'chrome', {headless: false,
      args: ['--start-fullscreen', '--window-position=0,0', '--window-size=1920,1080', '--mute-audio']});
    const desktopContext = await desktop.newContext({viewport: null});
    const content = await desktopContext.newPage();
    await content.setContent(`<style>body{margin:0;background:#18202b;overflow:hidden}canvas{width:100vw;height:100vh}</style>
      <canvas width="1920" height="1080"></canvas><script>
      const c=document.querySelector('canvas'),x=c.getContext('2d');
      function draw(t){x.fillStyle='#18202b';x.fillRect(0,0,1920,1080);
        for(let i=0;i<40;i++){x.fillStyle='hsl('+(i*9)+',65%,50%)';
          x.fillRect((t*.15+i*53)%2080-160,i*27,160,22)}
        x.fillStyle='white';x.font='42px sans-serif';x.fillText('Litecord 1080p60 desktop capture',80,100);
        requestAnimationFrame(draw)} requestAnimationFrame(draw);
      </script>`);
    result.desktop = await content.evaluate(() => ({screen: [screen.width, screen.height], viewport: [innerWidth, innerHeight]}));
    result.versions = {chrome: chrome.version(), node: process.version};

    async function sample(label, pages = [], attempt = 1) {
      if (quietBuilds) await waitForBuilds();
      await sleep(5000);
      const buildsBefore = await builds();
      const appLog = path.join(work, 'litecord_app.log');
      const logOffset = fs.statSync(appLog).size;
      const before = await Promise.all(pages.map(browsers.stats));
      const begin = Date.now();
      const wakePath = path.join(artifact, `${label}-${attempt}-wakeups.txt`);
      const [cpu, wake] = await Promise.allSettled([
        exec('python', [path.join(__dirname, 'process_sample.py'), String(source.pid), String(seconds)], {windowsHide: true, timeout: (seconds + 30) * 1000}),
        exec('powershell', ['-NoProfile', '-File', path.join(__dirname, 'wakeups.ps1'),
          '-ProcId', String(source.pid), '-Seconds', String(seconds), '-Label', label, '-Out', wakePath], {windowsHide: true, timeout: (seconds + 30) * 1000}),
      ]);
      if (cpu.status === 'rejected') throw cpu.reason;
      const after = await Promise.all(pages.map(browsers.stats));
      const elapsed = (Date.now() - begin) / 1000;
      const state = (await command({cmd: 'state'})).stream;
      const tx = [...fs.readFileSync(appLog).subarray(logOffset).toString('utf8')
        .matchAll(/\(TX\)\] FPS: ([\d.]+)\/60/g)].map(match => Number(match[1]));
      const wakeText = fs.existsSync(wakePath) ? fs.readFileSync(wakePath, 'utf8') : '';
      const buildsAfter = await builds();
      const phase = {label, attempt, ...JSON.parse(cpu.value.stdout), local_fps: state.local_fps,
        started_at: new Date(begin).toISOString(), builds_before: buildsBefore, builds_after: buildsAfter,
        web_viewers: state.web_viewers, web_active: state.web_active,
        native_peers: state.peers_seen.length,
        tx_fps_mean: tx.length ? Number((tx.reduce((a, b) => a + b) / tx.length).toFixed(2)) : null,
        wakeups_per_second: Number(wakeText.match(/total_wakeups_per_s=([\d.,]+)/)?.[1]?.replace(',', '.')) || null,
        wakeups_error: wake.status === 'rejected' ? wake.reason.message : undefined,
        playback: after.map((value, i) => ({...value, measured_fps: Number(((value.decoded - before[i].decoded) / elapsed).toFixed(2))}))};
      result.phases.push(phase); save(); console.log(JSON.stringify(phase));
      for (const page of phase.playback) {
        assert.equal(page.state, 'playing'); assert.equal(page.width, 1920); assert.equal(page.height, 1080);
        assert(page.measured_fps > 0, 'Video must advance throughout the sample');
      }
      if (quietBuilds && (buildsBefore.length || buildsAfter.length)) {
        assert(attempt < 3, 'Compilations interrupted three samples; repeat in a quiet window');
        console.log(`${label}: sample overlapped a compiler; keeping it and retrying`);
        await sample(label, pages, attempt + 1);
      }
    }
    let url;
    for (let round = 1; round <= rounds; round++) {
      await invoke({cmd: 'web_share_stop'});
      await sample(`${round}-web-off`);
      await invoke({cmd: 'web_share_start'});
      url = await waitFor(async () => {
        const stream = (await command({cmd: 'state'})).stream;
        if (stream.web_error) throw new Error(stream.web_error);
        return stream.web_active && stream.web_url;
      }, 'local web sharing');
      await sample(`${round}-web-zero`);
      const pages = [await watch(context, url)];
      await sample(`${round}-web-one`, pages);
      pages.push(await watch(context, url), await watch(context, url));
      await waitFor(async () => (await command({cmd: 'state'})).stream.web_viewers === 3, 'three viewers');
      await sample(`${round}-web-three`, pages);
      for (const page of pages) await page.close();
    }
    for (const name of ['edge', 'firefox']) {
      extra = await browsers.launch(api, name);
      const browserContext = await extra.newContext({locale: 'pt-BR'});
      await browsers.instrument(browserContext);
      const page = await watch(browserContext, url);
      const initial = await browsers.stats(page); await sleep(12000);
      const final = await browsers.stats(page);
      result.browsers.push({name, version: extra.version(), initial, ...final}); save();
      assert.equal(final.state, 'playing'); assert(final.decoded > initial.decoded + 100);
      assert.equal(final.width, 1920); assert.equal(final.height, 1080);
      console.log(`PASS: ${name} ${extra.version()} real 1080p capture`);
      await extra.close(); extra = undefined;
      await waitFor(async () => (await command({cmd: 'state'})).stream.web_viewers === 0, 'viewer cleanup');
    }
    await invoke({cmd: 'web_share_stop'});
    await sample('final-web-off');
  } catch (error) {
    result.error = error.message; save(); throw error;
  } finally {
    await extra?.close(); await chrome?.close(); await desktop?.close();
    if (controlsOwned && source.exitCode === null) {
      for (const cmd of ['web_share_stop', 'stop_screen_share', 'leave_voice']) {
        try { await command({cmd}); } catch {}
      }
    }
    if (source.exitCode === null) source.kill();
    if (source.exitCode === null) await new Promise(resolve => source.once('exit', resolve));
    log.end(); save();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
