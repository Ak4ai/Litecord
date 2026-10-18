# Performance and end-to-end test scripts

Windows only. They drive a test build through its localhost automation port:

```
cargo build --release --features automation
```

Never ship that build: it opens a control port and reads test variables (see
`src/automation.rs` and `src/utils/test_hooks.rs`).

## Idle cost of a running instance

```
powershell -File scripts\perf\wakeups.ps1 -ProcId <pid> -Seconds 60 -Label idle -Out wakeups.txt
```

Lists how often each thread wakes up per second, plus CPU time and memory.

## Voice and screen share end to end

Both tests start two instances side by side, each logged in with a different account
saved in the Litecord account list, and join them to the same voice channel. Neither
test changes which account the installed app opens with.

```
powershell -File scripts\perf\voice_e2e.ps1  -Sender "<name>" -Receiver "<name>" -Server "<server>" -Channel "<voice channel>"
powershell -File scripts\perf\stream_e2e.ps1 -Sender "<name>" -Receiver "<name>" -Server "<server>" -Channel "<voice channel>"
powershell -File scripts\perf\watch_on_demand.ps1 -Sender "<name>" -Receiver "<name>" -Server "<server>" -Channel "<voice channel>"
powershell -File scripts\perf\gateway_resume.ps1 -Sender "<name>" -Receiver "<name>" -Server "<server>" -Channel "<voice channel>"
powershell -File scripts\perf\soak.ps1 -Sender "<name>" -Receiver "<name>" -Server "<server>" -Channel "<voice channel>" -Minutes 120 -Cycle
```

- `voice_e2e.ps1` plays a synthesized reference speech into the sender (through a loopback
  of the default output device, so it needs one), and `voice_compare.py` checks that it
  arrived whole: send cadence, packets lost, and damaged 200 ms chunks. `-TimerPacing`
  sends on the old 20 ms timer, for A/B runs.
- `stream_e2e.ps1` shares the sender's screen and reports whether the receiver got it,
  plus the receiver's CPU and FPS while watching. `-ReceiverBackend winit-software`
  compares renderers; `-Fullscreen` watches at 1920x1080.
- `watch_on_demand.ps1` checks that nothing is received or encoded until the receiver
  presses Watch, that hiding the stream or going to the tray stops it, and how long the
  picture takes to come back ([design](../../docs/WATCH_ON_DEMAND.md)).
- `gateway_resume.ps1` drops the Discord gateway connection of both sides mid-stream and
  checks that nobody leaves the call.
- `soak.ps1` keeps a stream running for hours, cycling the tray and the popout, and
  samples CPU and memory on both sides into a CSV.

## Browser sharing without Discord accounts

See [the web-sharing guide](../../docs/WEB_SHARING.md). `web_share_e2e.cjs` drives
a synthetic Rust source and headless Chrome against a local Wrangler worker.
It verifies H.264/Opus playback, access control, the viewer limit, and cleanup.
`process_sample.py <pid> [seconds]` samples CPU and memory on Windows using only
Python's standard library; CPU is reported as percent of one logical core.
