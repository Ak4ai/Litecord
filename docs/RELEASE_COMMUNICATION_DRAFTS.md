# 1. Issue para o Slint (github.com/slint-ui/slint/issues/new)

**Title:** winit backend keeps waking the event loop forever when an animation runs while the window is hidden

**Body:**

Slint 1.17.1 and 1.18.1, winit backend, Windows 11 (femtovg and software renderers).

We hide our main window to the tray with `window.with_winit_window(|w| w.set_visible(false))`. If any animated property changes while it is hidden (a list row with `animate background`, a scroll, etc.), the UI thread never goes back to sleep until the window is shown again:

- `about_to_wait` calls `request_redraw()` for every window that `has_active_animations()`, visible or not. A hidden window never paints, the animation never advances, and the loop wakes ~130 times per second.
- With that skipped, `TimerBasedFrameThrottle` keeps its repeated timer running at the monitor rate (~32/s here), because `pending_redraw()` stays true for a window that never paints.

We measured this with per-thread context switches: 132 wakeups/s on the UI thread for as long as the app stayed in the tray (hours, for a user playing a game). Skipping windows whose winit `is_visible()` is `Some(false)` in both places fixes it, and the window repaints correctly when shown again:

```rust
// event_loop.rs, about_to_wait
if w.window().has_active_animations()
    && w.winit_window().is_none_or(|ww| ww.is_visible() != Some(false))
{
    w.request_redraw();
}

// frame_throttle.rs, TimerBasedFrameThrottle
if self.timer.running() || winit_window.is_visible() == Some(false) { return; }
// and in the timer callback:
let keep_running = window_adapter.pending_redraw()
    && window_adapter.winit_window().is_none_or(|w| w.is_visible() != Some(false));
```

A related one in core: `ConstantDeceleration::step_internal` can end up with a subnormal velocity; `|velocity / deceleration|` then underflows to 0, every later step has zero duration and the animation never finishes. Reproduced with `ConstantDecelerationParameters::new_with_distance(-50., 0.18)` stepped every 16 ms. We treat a zero time-to-stop as finished:

```rust
let time_to_stop = f32::abs(self.velocity / self.data.deceleration);
if time_to_stop == 0. {
    self.velocity = 0.;
    return true;
}
```

Happy to open a PR with both changes and the unit test if that helps.

---

# 2. Comentário na issue #27 (quando o build da v1.2.1 estiver disponível)

Thanks for the report, it led to the main change in this version.

We measured how often each Litecord thread wakes up while the app sits in the tray, and found the problem: as soon as anything in the interface animated while the window was hidden (someone joining a voice channel, an unread counter changing), the UI kept redrawing about 130 times per second until the window was opened again. That would fit what you described (CPU near 0%, yet less stable frame times even in the tray), though we could not reproduce it on your hardware. It is fixed in 1.2.1, and the interface is now drawn on the CPU by default, so Litecord no longer keeps a GPU context or graphics driver threads running beside your game (GPU rendering is still available in Settings → Video & Interface).

On our machine Litecord now wakes up less than once per second in the tray (it was 338 times per second) and stays at 0.00% CPU. During calls the voice is paced by the microphone, so Litecord no longer needs the 1 ms Windows timer.

We only had Intel and NVIDIA GPUs to test with, so your 9070 XT matters a lot here: could you try <link to the build> in the same games and tell us whether the frame time is steadier? Comparing with Litecord closed is the most useful baseline.
