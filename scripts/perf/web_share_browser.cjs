// Shared browser drivers for local web-sharing validation. Uses installed browsers.
const fs = require('node:fs');
const path = require('node:path');

function drivers() {
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
  throw new Error('Playwright must already be installed or cached');
}

async function launch(api, name, options = {}) {
  if (!['chrome', 'edge', 'firefox'].includes(name)) throw new Error(`Unknown browser: ${name}`);
  if (name === 'firefox') return api.firefox.launch({channel: 'moz-firefox', headless: true, ...options});
  const executablePath = name === 'edge'
    ? 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe'
    : 'C:/Program Files/Google/Chrome/Application/chrome.exe';
  return api.chromium.launch({executablePath, headless: true,
    args: ['--autoplay-policy=no-user-gesture-required', '--mute-audio'], ...options});
}

async function instrument(context) {
  await context.addInitScript(() => {
    window.__peers = [];
    const Real = window.RTCPeerConnection;
    window.RTCPeerConnection = class extends Real {
      constructor(...args) { super(...args); window.__peers.push(this); }
    };
  });
}

async function stats(page) {
  return page.evaluate(async () => {
    const records = [], candidates = [];
    for (const pc of window.__peers || []) {
      const all = await pc.getStats();
      for (const entry of all.values()) {
        if (entry.type === 'inbound-rtp') records.push({kind: entry.kind ?? entry.mediaType,
          bytes: entry.bytesReceived, frames: entry.framesDecoded, dropped: entry.framesDropped,
          width: entry.frameWidth, height: entry.frameHeight, fps: entry.framesPerSecond,
          samples: entry.totalSamplesReceived, energy: entry.totalAudioEnergy,
          codec: all.get(entry.codecId)?.mimeType});
        if (entry.type === 'candidate-pair' && entry.state === 'succeeded' && entry.nominated) {
          const remote = all.get(entry.remoteCandidateId);
          const address = remote?.address ?? remote?.ip;
          candidates.push({local: all.get(entry.localCandidateId)?.candidateType,
            remote: remote?.candidateType,
            remote_loopback: address === undefined ? undefined : address === '::1' || address.startsWith('127.')});
        }
      }
    }
    const video = document.querySelector('video');
    return {state: document.querySelector('#status').dataset.state, records, candidates,
      width: video.videoWidth, height: video.videoHeight,
      decoded: records.find(record => record.kind === 'video')?.frames,
      playback_frames: video.getVideoPlaybackQuality?.().totalVideoFrames,
      current_time: video.currentTime};
  });
}

module.exports = {drivers, launch, instrument, stats};
