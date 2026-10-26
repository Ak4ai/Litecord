//! Encoding the stream's pictures on a thread of their own.
//!
//! The send thread used to hand each picture to the encoder itself. On a hybrid laptop,
//! while the other GPU is busy, the display GPU's copy into the encoder and its
//! `ProcessInput` take 50-500 ms now and then, and the whole send loop (pacing, preview,
//! network) waited behind them. Here the send thread only leaves the newest picture in a
//! one-slot mailbox and goes on: a picture still waiting when a newer one arrives is
//! dropped, as a full queue would, and a slow encoder costs frames instead of freezing
//! everything else.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use log::warn;

/// A turn of the encoder thread slower than this is logged, with the pictures dropped.
const SLOW_TURN: Duration = Duration::from_millis(50);
/// How often the encoder thread reports trouble.
const REPORT_EVERY: Duration = Duration::from_secs(1);

struct Shared<T> {
    pending: Mutex<Option<T>>,
    wake: Condvar,
    stop: AtomicBool,
    /// Pictures replaced in the mailbox before the encoder took them.
    dropped: AtomicU64,
}

/// Runs `work` on its own thread over the newest item submitted.
pub struct LatestWorker<T> {
    shared: Arc<Shared<T>>,
    done: Receiver<Result<Vec<u8>, String>>,
    thread: Option<JoinHandle<()>>,
}

impl<T: Send + 'static> LatestWorker<T> {
    /// `work` turns an item into an access unit; an empty one (nothing finished yet) is
    /// not delivered.
    pub fn spawn(name: &str, mut work: impl FnMut(&T) -> Result<Vec<u8>, String> + Send + 'static) -> Self {
        let shared = Arc::new(Shared { pending: Mutex::new(None), wake: Condvar::new(), stop: AtomicBool::new(false), dropped: AtomicU64::new(0) });
        let (tx, done) = channel();
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name(name.to_string())
                .spawn(move || {
                    let mut report = Instant::now();
                    let (mut turns, mut slowest) = (0u32, Duration::ZERO);
                    loop {
                        let item = {
                            let mut slot = shared.pending.lock().unwrap_or_else(|e| e.into_inner());
                            loop {
                                if shared.stop.load(Ordering::Acquire) {
                                    return;
                                }
                                if let Some(item) = slot.take() {
                                    break item;
                                }
                                slot = shared.wake.wait(slot).unwrap_or_else(|e| e.into_inner());
                            }
                        };
                        let started = Instant::now();
                        let result = work(&item);
                        // Frees the picture's texture for the capture before anything else.
                        drop(item);
                        let took = started.elapsed();
                        turns += 1;
                        slowest = slowest.max(took);
                        if report.elapsed() >= REPORT_EVERY {
                            let dropped = shared.dropped.swap(0, Ordering::Relaxed);
                            if slowest >= SLOW_TURN || dropped > 0 {
                                warn!("🐢 [GPU ENCODE] Último segundo: {} quadros codificados, {} descartados por o codificador não acompanhar, mais lento {:.1} ms",
                                    turns, dropped, slowest.as_secs_f64() * 1e3);
                            }
                            report = Instant::now();
                            turns = 0;
                            slowest = Duration::ZERO;
                        }
                        match result {
                            Ok(bytes) if bytes.is_empty() => {}
                            other => {
                                if tx.send(other).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                })
                .expect("Falha ao iniciar thread do codificador")
        };
        Self { shared, done, thread: Some(thread) }
    }

    /// Leaves `item` for the encoder, replacing one it has not started on yet. Never waits
    /// for the encoder.
    pub fn submit(&self, item: T) {
        let replaced = self.shared.pending.lock().unwrap_or_else(|e| e.into_inner()).replace(item).is_some();
        if replaced {
            self.shared.dropped.fetch_add(1, Ordering::Relaxed);
        }
        self.shared.wake.notify_one();
    }

    /// The next finished access unit, waiting up to `wait` for one.
    pub fn next(&self, wait: Duration) -> Option<Result<Vec<u8>, String>> {
        match self.done.recv_timeout(wait) {
            Ok(r) => Some(r),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Err("a thread do codificador terminou".into())),
        }
    }
}

impl<T> Drop for LatestWorker<T> {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.wake.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(windows)]
pub use gpu::GpuEncodeWorker;

#[cfg(windows)]
mod gpu {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    use super::LatestWorker;
    use crate::encoder::GpuVideoEncoder;
    use crate::screen_capture::gpu_tx::GpuFrame;

    /// A texture encoder running on its own thread (see the module notes).
    pub struct GpuEncodeWorker {
        inner: LatestWorker<GpuFrame>,
        idr: Arc<AtomicBool>,
        /// 0 until the first request.
        bitrate: Arc<AtomicU32>,
    }

    impl GpuEncodeWorker {
        pub fn new(mut encoder: Box<dyn GpuVideoEncoder>) -> Self {
            let idr = Arc::new(AtomicBool::new(false));
            let bitrate = Arc::new(AtomicU32::new(0));
            let (idr_seen, bitrate_seen) = (idr.clone(), bitrate.clone());
            let mut applied = 0u32;
            let mut com_ready = false;
            let inner = LatestWorker::spawn("gpu-encode", move |frame: &GpuFrame| {
                if !com_ready {
                    // Media Foundation encoders expect a multithreaded apartment.
                    unsafe {
                        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                    }
                    com_ready = true;
                }
                let wanted = bitrate_seen.load(Ordering::Relaxed);
                if wanted != 0 && wanted != applied {
                    encoder.set_bitrate(wanted);
                    applied = wanted;
                }
                if idr_seen.swap(false, Ordering::Relaxed) {
                    encoder.force_idr();
                }
                encoder.encode_texture(&frame.texture)
            });
            Self { inner, idr, bitrate }
        }

        pub fn set_bitrate(&self, bitrate_bps: u32) {
            self.bitrate.store(bitrate_bps, Ordering::Relaxed);
        }

        pub fn force_idr(&self) {
            self.idr.store(true, Ordering::Relaxed);
        }

        /// Hands the picture over (the slot of its texture stays taken until the encoder
        /// is done with it).
        pub fn submit(&self, frame: &GpuFrame) {
            self.inner.submit(frame.clone());
        }

        pub fn next_output(&self, wait: Duration) -> Option<Result<Vec<u8>, String>> {
            self.inner.next(wait)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slow_encoder_does_not_hold_up_the_submitter_and_gets_the_newest_item() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let worker = LatestWorker::spawn("test-encode", move |n: &u32| {
            std::thread::sleep(Duration::from_millis(40));
            log.lock().unwrap().push(*n);
            Ok(vec![*n as u8])
        });
        let mut worst = Duration::ZERO;
        for n in 1..=30u32 {
            let t = Instant::now();
            worker.submit(n);
            worst = worst.max(t.elapsed());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(worst < Duration::from_millis(10), "submit waited {:?}", worst);
        // The last picture is never lost, the ones in between were dropped.
        let mut last = None;
        while let Some(r) = worker.next(Duration::from_millis(300)) {
            last = Some(r.unwrap()[0]);
        }
        assert_eq!(last, Some(30));
        let seen = seen.lock().unwrap();
        assert!(seen.len() < 30, "nothing was dropped: {}", seen.len());
        assert!(seen.windows(2).all(|w| w[0] < w[1]), "out of order: {:?}", seen);
    }

    #[test]
    fn a_fast_encoder_delivers_every_item_in_order() {
        let worker = LatestWorker::spawn("test-encode", |n: &u32| Ok(vec![*n as u8]));
        for n in 1..=5u32 {
            worker.submit(n);
            assert_eq!(worker.next(Duration::from_millis(500)).unwrap().unwrap(), vec![n as u8]);
        }
    }

    #[test]
    fn empty_output_is_not_delivered_and_errors_are() {
        let worker = LatestWorker::spawn("test-encode", |n: &u32| if *n == 1 { Ok(Vec::new()) } else { Err("boom".to_string()) });
        worker.submit(1);
        assert!(worker.next(Duration::from_millis(100)).is_none());
        worker.submit(2);
        assert_eq!(worker.next(Duration::from_millis(500)).unwrap().unwrap_err(), "boom");
    }

    #[test]
    fn dropping_an_idle_worker_stops_its_thread() {
        let t = Instant::now();
        drop(LatestWorker::<u32>::spawn("test-encode", |_| Ok(vec![1])));
        assert!(t.elapsed() < Duration::from_millis(500));
    }
}
