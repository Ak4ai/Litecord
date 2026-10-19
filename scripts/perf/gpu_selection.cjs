// Real Windows capture with the installed GPUs, CPU encoding and a missing saved GPU.
// Uses only the two authorized accounts/channel; never writes their saved token vault.
// Start the local signaling worker first, then use a release automation build.
// Optional --baseline=<exe> measures the integrated build under the same workload.
const fs = require('node:fs');
const path = require('node:path');
const net = require('node:net');
const assert = require('node:assert/strict');
const {spawn, execFile} = require('node:child_process');
const {promisify} = require('node:util');
const browsers = require('./web_share_browser.cjs');
const exec = promisify(execFile);
const root = path.resolve(__dirname, '../..');
const arg = (name, fallback) => process.argv.find(a => a.startsWith(`--${name}=`))?.slice(name.length + 3) ?? fallback;
const exe = path.resolve(arg('exe', path.join(root, 'target/release/litecord.exe')));
const baseline = arg('baseline', '');
const seconds = Number(arg('seconds', '10'));
assert(Number.isInteger(seconds) && seconds >= 5);
const worker = process.env.LITECORD_SIGNALING_URL || 'ws://127.0.0.1:8787';
assert(['127.0.0.1', 'localhost'].includes(new URL(worker).hostname), 'Use a local test worker');
const out = path.join(root, 'target/gpu-selection', new Date().toISOString().replace(/[:.]/g, '-'));
fs.mkdirSync(out, {recursive:true});
const results = {phases:[], screenshots:[], stops:[]};
const owned = new Map();
const sleep = ms => new Promise(r => setTimeout(r, ms));
const save = () => fs.writeFileSync(path.join(out, 'results.json'), JSON.stringify(results, null, 2));
function raw(port, value) {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection({host:'127.0.0.1', port});
    let data = '';
    socket.setTimeout(20000, () => socket.destroy(new Error('Automation timeout')));
    socket.on('error', reject);
    socket.on('connect', () => socket.write(JSON.stringify(value) + '\n'));
    socket.on('end', () => { if (!data.includes('\n')) reject(new Error('Automation disconnected')); });
    socket.on('data', chunk => {
      data += chunk;
      if (data.includes('\n')) {
        socket.end();
        try { resolve(JSON.parse(data.split('\n')[0])); } catch (e) { reject(e); }
      }
    });
  });
}
async function cmd(port, value) {
  assert(owned.has(port), 'Only control an instance started by this test');
  const start = performance.now();
  const result = await raw(port, value);
  assert(result.ok, result.error);
  if (value.cmd === 'stop_screen_share') results.stops.push({port,milliseconds:performance.now()-start});
  return result;
}
async function until(fn, label, timeout=45000) {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    try { const value = await fn(); if (value) return value; } catch {}
    await sleep(250);
  }
  throw new Error(`Timed out: ${label}`);
}
async function launch(binary, account, port, label, encoder='auto') {
  const occupied = await raw(port, {cmd:'size'}).then(()=>true, ()=>false);
  assert(!occupied, `Automation port ${port} is already occupied`);
  const work = path.join(out, label);
  fs.mkdirSync(work, {recursive:true});
  fs.writeFileSync(path.join(work, '.litecord_video_settings.json'), JSON.stringify({
    video_encoder:encoder, video_capture_backend:'auto', audio_loopback_backend:'auto',
    enable_self_preview_notice:false, show_hardware_monitor:false, use_gpu_renderer:false
  }));
  const proc = spawn(binary, [], {cwd:work, windowsHide:true, stdio:'ignore', env:{...process.env,
    PATH:root + path.delimiter + process.env.PATH, LITECORD_TEST_ACCOUNT:account,
    LITECORD_TEST_MUTE_OUTPUT:'1', LITECORD_AUTOMATION_PORT:String(port), LITECORD_SIGNALING_URL:worker
  }});
  owned.set(port, proc);
  await until(async () => {
    assert(proc.exitCode === null, `${label} exited`);
    const s = await cmd(port, {cmd:'state'});
    return s.is_logged_in && s.user_id !== '0' && s;
  }, `${label} login`, 90000);
  return proc;
}
async function close(port) {
  const proc = owned.get(port);
  if (!proc) return;
  await cmd(port, {cmd:'stop_screen_share'}).catch(()=>{});
  await cmd(port, {cmd:'leave_voice'}).catch(()=>{});
  await sleep(2000);
  if (proc.exitCode === null) proc.kill();
  owned.delete(port);
}
async function join(port) {
  await cmd(port, {cmd:'open_guild', name:'Ricogames_ e MariaJulia_'});
  await sleep(4000);
  const r = await cmd(port, {cmd:'open_channel', name:'fofocas'});
  assert(r.is_voice);
  await until(async () => {
    const s = await cmd(port, {cmd:'state'});
    return s.voice.is_in_voice && !s.voice.is_connecting && s.voice.channel === 'fofocas';
  }, 'authorized voice channel');
}
async function measure(proc, label, streaming=false, expected=null) {
  const before = await cmd(47232, {cmd:'state'});
  const receiveStart = performance.now();
  // Cycle accounting avoids Windows CPU-time sampling missing short GPU/driver work.
  const ps = `Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public class C { [DllImport("kernel32.dll")] public static extern bool QueryProcessCycleTime(IntPtr h,out ulong c); }';
  $p=Get-Process -Id ${proc.pid}; [UInt64]$a=0; [UInt64]$b=0; [C]::QueryProcessCycleTime($p.Handle,[ref]$a)|Out-Null;
  $t=[Diagnostics.Stopwatch]::StartNew(); Start-Sleep -Seconds ${seconds}; [C]::QueryProcessCycleTime($p.Handle,[ref]$b)|Out-Null; $t.Stop(); $p.Refresh();
  @{core_percent=($b-$a)/$t.Elapsed.TotalSeconds/30000000; private_mb=$p.PrivateMemorySize64/1MB; working_set_mb=$p.WorkingSet64/1MB; seconds=$t.Elapsed.TotalSeconds}|ConvertTo-Json -Compress`;
  const {stdout} = await exec('powershell.exe', ['-NoProfile','-Command',ps], {windowsHide:true});
  const after = await cmd(47232, {cmd:'state'});
  const receiveSeconds = (performance.now()-receiveStart)/1000;
  const sender = await cmd(47231, {cmd:'state'});
  assert(sender.voice.is_in_voice && sender.voice.channel === 'fofocas');
  const phase = {label,...JSON.parse(stdout),pictures:after.stream.frames_shown-before.stream.frames_shown,
    receive_seconds:receiveSeconds, received_fps:(after.stream.frames_shown-before.stream.frames_shown)/receiveSeconds,
    remote_fps:after.stream.remote_fps,local_fps:sender.stream.local_fps};
  if (expected !== null) {
    phase.selection = await cmd(47231, {cmd:'video_encoder_options'});
    if (expected) {
      assert(/DXGI|WGC/.test(phase.selection.capture), `No capture backend: ${phase.selection.capture}`);
      if (expected.gpu) assert(phase.selection.capture.includes(expected.gpu), `Wrong GPU: ${phase.selection.capture}`);
      assert(phase.selection.gpu_path);
    }
    else assert(!phase.selection.gpu_path, 'CPU choice must not encode textures');
  }
  if (streaming) assert(phase.pictures >= seconds*10, `Stream did not advance: ${phase.pictures} pictures`);
  else assert.equal(phase.pictures, 0, 'No pictures may arrive without a stream/watch');
  results.phases.push(phase); save();
  console.log(`${label}: ${phase.core_percent.toFixed(1)}% of one 3 GHz core; ${phase.pictures} pictures; received ${phase.received_fps.toFixed(1)} FPS`);
}
async function startAndWatch(proc, label, expected=null, windowTitle=null) {
  const state = await cmd(47231, {cmd:'state'});
  const initial = (await cmd(47232, {cmd:'state'})).stream.frames_shown;
  await cmd(47231, {cmd:'start_screen_share',res:'1080p',fps:60,screen:0,audio:false,
    ...(windowTitle ? {window_title:windowTitle} : {})});
  await sleep(2500);
  await cmd(47232, {cmd:'watch_stream',user_id:state.user_id});
  await until(async () => (await cmd(47232, {cmd:'state'})).stream.frames_shown > initial + 10, label);
  await sleep(2500);
  await measure(proc, label, true, expected);
}
let desktop;
(async () => {
  await launch(exe, 'Henrique', 47232, 'receiver');
  await join(47232);
  desktop = await browsers.launch(browsers.drivers().browser, 'chrome', {headless:false,
    args:['--start-fullscreen','--window-position=0,0','--window-size=1920,1080','--mute-audio']});
  const page = await desktop.newPage({viewport:null});
  await page.setContent(`<title>Litecord GPU capture test</title><style>body{margin:0;background:#18202b}canvas{width:100vw;height:100vh}</style><canvas width="1920" height="1080"></canvas>
    <script>let x=document.querySelector('canvas').getContext('2d');function draw(t){x.fillStyle='#18202b';x.fillRect(0,0,1920,1080);for(let i=0;i<35;i++){x.fillStyle='hsl('+i*10+',65%,50%)';x.fillRect((t*.15+i*53)%2080-160,i*28,160,22)}requestAnimationFrame(draw)}requestAnimationFrame(draw)</script>`);
  if (baseline) {
    const proc = await launch(path.resolve(baseline), 'Marido', 47231, 'baseline');
    await join(47231);
    await cmd(47231, {cmd:'minimize_to_tray'});
    await measure(proc,'baseline idle');
    await startAndWatch(proc,'baseline automatic');
    await close(47231);
    await sleep(3000);
  }
  const proc = await launch(exe, 'Marido', 47231, 'sender', 'gpu:dead:beef');
  await join(47231);
  await cmd(47231, {cmd:'set_language',code:'en'});
  let opts = await cmd(47231, {cmd:'video_encoder_options'});
  results.options = opts.options;
  assert(opts.options.some(o=>o.id === 'gpu:dead:beef' && o.selected), 'Missing saved GPU must stay visible/selected');
  const gpus = opts.options.filter(o=>o.id.startsWith('gpu:') && o.id !== 'gpu:dead:beef');
  await cmd(47231, {cmd:'minimize_to_tray'});
  await measure(proc,'new idle');
  await startAndWatch(proc,'missing GPU falls back to automatic',{});
  await cmd(47231, {cmd:'select_video_encoder',id:'openh264'});
  opts = await cmd(47231, {cmd:'video_encoder_options'});
  assert.equal(opts.stream_choice, 'gpu:dead:beef', 'Changing the setting must not switch a running stream');
  assert(opts.gpu_path && /DXGI|WGC/.test(opts.capture));
  await cmd(47231, {cmd:'stop_screen_share'});
  await sleep(2000);
  assert.equal((await cmd(47231, {cmd:'video_encoder_options'})).capture, '', 'Stopping clears actual capture status');
  for (const option of [{id:'auto'},...gpus,{id:'openh264'}]) {
    await cmd(47231, {cmd:'select_video_encoder',id:option.id});
    const expected = option.id === 'openh264' ? '' : {gpu:option.name};
    await startAndWatch(proc,option.id,expected);
    await cmd(47231, {cmd:'stop_screen_share'});
    await sleep(2000);
    assert.equal((await cmd(47231, {cmd:'video_encoder_options'})).capture, '');
    if (option.id.startsWith('gpu:10de:')) await measure(proc,'NVIDIA stopped');
  }
  await cmd(47231, {cmd:'select_video_encoder',id:'auto'});
  await startAndWatch(proc,'automatic window capture',{},'Litecord GPU capture test');
  assert((await cmd(47231,{cmd:'video_encoder_options'})).capture.includes('WGC'), 'Window capture must use WGC');
  await cmd(47231, {cmd:'stop_screen_share'});
  await sleep(2000);
  assert(!(await cmd(47231,{cmd:'video_encoder_options'})).gpu_path, 'Stopping must release the GPU path');
  await cmd(47231, {cmd:'restore'});
  await cmd(47231, {cmd:'set',prop:'show_settings_modal',value:true});
  await cmd(47231, {cmd:'set',prop:'settings_active_tab',value:5});
  for (const code of ['en','pt','es','de','fr','ru','ja']) {
    await cmd(47231, {cmd:'set_language',code});
    await sleep(250);
    const values = await cmd(47231, {cmd:'video_encoder_options'});
    assert(values.options.every(o=>o.description && o.name));
    const expected = {en:'Automatic',pt:'Automático',es:'Automático',de:'Automatisch',fr:'Automatique',ru:'Автоматически',ja:'自動'};
    assert(values.options.find(o=>o.id === 'auto').name.startsWith(expected[code]), `GPU choices did not change to ${code}`);
    const screenshot = path.join(out,`settings-${code}.png`);
    await cmd(47231, {cmd:'screenshot',path:screenshot});
    results.screenshots.push(screenshot);
  }
  assert(results.stops.every(s=>s.milliseconds < 500), 'Stopping blocked the UI');
  results.pass = true; save(); console.log(`PASS; artifacts: ${out}`);
})().catch(e=>{results.error=e.stack;save();console.error(e);process.exitCode=1;}).finally(async()=>{
  await close(47231); await close(47232); await desktop?.close();
});
