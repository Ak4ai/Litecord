//! Watching on demand (protocol 3, see docs/WATCH_ON_DEMAND.md).
//!
//! Sender side: the viewers that asked for the stream in the last few seconds, the
//! stream's own key, and the bitrate decided by the worst of them. Viewer side: which
//! streams the user wants and the keys their senders handed over.
//!
//! The rules live in plain structs (`Viewers`, `RateControl`) so they can be tested;
//! the statics below are the one instance the capture threads share.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A watch request keeps the viewer on the list this long.
pub const LEASE: Duration = Duration::from_secs(4);
/// Viewers renew their request this often.
pub const RENEW_EVERY: Duration = Duration::from_secs(1);
/// Loss reports older than this no longer count for the bitrate.
const LOSS_REPORT_AGE: Duration = Duration::from_secs(3);

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// On the list; `new` when the viewer was not watching a moment ago (it needs a keyframe).
    Accepted { new: bool },
    /// Not in the sender's voice channel.
    NotInChannel,
    /// Someone already watches under this user id with another key.
    KeyMismatch,
}

struct Viewer {
    addr: SocketAddr,
    last_seen: Instant,
    loss: Option<(u16, Instant)>,
}

/// The viewers of one stream.
pub struct Viewers {
    list: HashMap<u64, Viewer>,
    /// The first public key each user id watched with during this stream.
    bound: HashMap<u64, [u8; 32]>,
}

impl Viewers {
    pub fn new() -> Self {
        Self { list: HashMap::new(), bound: HashMap::new() }
    }

    pub fn watch(&mut self, uid: u64, public: [u8; 32], addr: SocketAddr, in_channel: bool, now: Instant) -> Verdict {
        if !in_channel {
            return Verdict::NotInChannel;
        }
        if *self.bound.entry(uid).or_insert(public) != public {
            return Verdict::KeyMismatch;
        }
        let new = self.list.get(&uid).map_or(true, |v| now.duration_since(v.last_seen) >= LEASE);
        let loss = self.list.get(&uid).and_then(|v| v.loss);
        self.list.insert(uid, Viewer { addr, last_seen: now, loss });
        Verdict::Accepted { new }
    }

    /// Whether `uid` was watching.
    pub fn unwatch(&mut self, uid: u64) -> bool {
        self.list.remove(&uid).is_some()
    }

    /// Drops the viewers whose request expired.
    pub fn expire(&mut self, now: Instant) {
        self.list.retain(|_, v| now.duration_since(v.last_seen) < LEASE);
    }

    pub fn addrs(&self) -> Vec<SocketAddr> {
        self.list.values().map(|v| v.addr).collect()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn report_loss(&mut self, uid: u64, permille: u16, now: Instant) {
        if let Some(v) = self.list.get_mut(&uid) {
            v.loss = Some((permille, now));
        }
    }

    /// The highest loss a current viewer reported recently.
    pub fn worst_loss(&self, now: Instant) -> Option<u16> {
        self.list
            .values()
            .filter_map(|v| v.loss.filter(|(_, at)| now.duration_since(*at) < LOSS_REPORT_AGE).map(|(l, _)| l))
            .max()
    }
}

/// Bitrate of one stream: down 20% on heavy loss, back up slowly while it is clean.
pub struct RateControl {
    max: u32,
    current: u32,
    clean_since: Option<Instant>,
    last_cut: Option<Instant>,
}

pub const MIN_BITRATE: u32 = 3_000_000;
const STEP_UP: u32 = 500_000;
const CLEAN_FOR: Duration = Duration::from_secs(5);
const HEAVY_LOSS_PERMILLE: u16 = 80;
const CLEAN_LOSS_PERMILLE: u16 = 20;

impl RateControl {
    pub fn new(max: u32) -> Self {
        Self { max, current: max, clean_since: None, last_cut: None }
    }

    pub fn current(&self) -> u32 {
        self.current
    }

    /// `worst_loss`: the worst recent report among viewers, if any.
    pub fn update(&mut self, worst_loss: Option<u16>, now: Instant) -> u32 {
        match worst_loss {
            Some(l) if l > HEAVY_LOSS_PERMILLE => {
                self.clean_since = None;
                // One cut per second: reports from several viewers describe the same congestion.
                if self.last_cut.map_or(true, |t| now.duration_since(t) >= Duration::from_secs(1)) {
                    self.current = ((self.current as u64 * 4 / 5) as u32).max(MIN_BITRATE.min(self.max));
                    self.last_cut = Some(now);
                }
            }
            Some(l) if l < CLEAN_LOSS_PERMILLE => {
                let since = *self.clean_since.get_or_insert(now);
                if now.duration_since(since) >= CLEAN_FOR && self.current < self.max {
                    self.current = (self.current + STEP_UP).min(self.max);
                    self.clean_since = Some(now);
                }
            }
            Some(_) => self.clean_since = None,
            // Nobody reporting (no viewers, or no losses measured yet): hold.
            None => {}
        }
        self.current
    }
}

// ---- Sender: the stream being sent -------------------------------------------------

struct Session {
    key: [u8; 32],
    viewers: Viewers,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

/// A new stream: new key, nobody watching.
pub fn start_session() {
    *SESSION.lock().unwrap_or_else(|e| e.into_inner()) = Some(Session { key: rand::random(), viewers: Viewers::new() });
}

pub fn end_session() {
    *SESSION.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The key this stream's media is encrypted with.
pub fn session_key() -> Option<[u8; 32]> {
    SESSION.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.key)
}

/// A watch request; `None` when not streaming.
pub fn on_watch(uid: u64, public: [u8; 32], addr: SocketAddr, in_channel: bool) -> Option<Verdict> {
    let mut s = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    Some(s.as_mut()?.viewers.watch(uid, public, addr, in_channel, Instant::now()))
}

/// Whether `uid` was watching (UNWATCH is sent three times).
pub fn on_unwatch(uid: u64) -> bool {
    SESSION.lock().unwrap_or_else(|e| e.into_inner()).as_mut().is_some_and(|s| s.viewers.unwatch(uid))
}

pub fn on_loss_report(uid: u64, permille: u16) {
    if let Some(s) = SESSION.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        s.viewers.report_loss(uid, permille, Instant::now());
    }
}

/// Where to send the stream now (expired viewers dropped).
pub fn viewer_addrs() -> Vec<SocketAddr> {
    let mut s = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    match s.as_mut() {
        Some(s) => {
            s.viewers.expire(Instant::now());
            s.viewers.addrs()
        }
        None => Vec::new(),
    }
}

pub fn worst_loss() -> Option<u16> {
    SESSION.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|s| s.viewers.worst_loss(Instant::now()))
}

// ---- Viewer: the streams the user wants ---------------------------------------------

static WANTED: Mutex<Option<HashSet<u64>>> = Mutex::new(None);
static STREAM_KEYS: Mutex<Option<HashMap<u64, [u8; 32]>>> = Mutex::new(None);

/// The user asked to watch `uid`'s stream (Watch) or stopped (close, hide).
pub fn set_wanted(uid: u64, on: bool) {
    let mut w = WANTED.lock().unwrap_or_else(|e| e.into_inner());
    let set = w.get_or_insert_with(HashSet::new);
    let changed = if on { set.insert(uid) } else { set.remove(&uid) };
    drop(w);
    if changed {
        // The receive thread sends the request (or the UNWATCH) now, not on its next wake.
        super::types::wake_p2p_receiver();
    }
}

/// Left the call: nothing is wanted any more.
pub fn clear_wanted() {
    *WANTED.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub fn is_wanted(uid: u64) -> bool {
    WANTED.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|s| s.contains(&uid))
}

/// The streams to ask for now: wanted, and some window can show them.
pub fn streams_to_request() -> Vec<u64> {
    if !super::video_pipeline::streams_watched() {
        return Vec::new();
    }
    WANTED.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.iter().copied().collect()).unwrap_or_default()
}

pub fn set_stream_key(uid: u64, key: [u8; 32]) {
    STREAM_KEYS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(uid, key);
}

pub fn stream_key(uid: u64) -> Option<[u8; 32]> {
    STREAM_KEYS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&uid).copied())
}

/// The peer's stream ended: its key is useless now.
pub fn forget_stream(uid: u64) {
    if let Some(m) = STREAM_KEYS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        m.remove(&uid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from(([10, 0, 0, 1], port))
    }

    #[test]
    fn only_people_in_the_voice_channel_can_watch() {
        let mut v = Viewers::new();
        let now = Instant::now();
        assert_eq!(v.watch(1, [1; 32], addr(1), false, now), Verdict::NotInChannel);
        assert_eq!(v.len(), 0);
        assert_eq!(v.watch(1, [1; 32], addr(1), true, now), Verdict::Accepted { new: true });
        assert_eq!(v.addrs(), vec![addr(1)]);
    }

    #[test]
    fn a_user_id_keeps_the_key_it_first_watched_with() {
        let mut v = Viewers::new();
        let now = Instant::now();
        assert_eq!(v.watch(1, [1; 32], addr(1), true, now), Verdict::Accepted { new: true });
        assert_eq!(v.watch(1, [2; 32], addr(9), true, now), Verdict::KeyMismatch);
        assert_eq!(v.addrs(), vec![addr(1)]);
        // Even after leaving: the stream's key went to the first one.
        v.unwatch(1);
        assert_eq!(v.watch(1, [2; 32], addr(9), true, now), Verdict::KeyMismatch);
    }

    #[test]
    fn requests_expire_unless_renewed_and_a_return_counts_as_new() {
        let mut v = Viewers::new();
        let t0 = Instant::now();
        v.watch(1, [1; 32], addr(1), true, t0);
        assert_eq!(v.watch(1, [1; 32], addr(1), true, t0 + RENEW_EVERY), Verdict::Accepted { new: false });
        v.expire(t0 + RENEW_EVERY + LEASE - Duration::from_millis(1));
        assert_eq!(v.len(), 1);
        v.expire(t0 + RENEW_EVERY + LEASE);
        assert_eq!(v.len(), 0);
        assert_eq!(v.watch(1, [1; 32], addr(1), true, t0 + Duration::from_secs(20)), Verdict::Accepted { new: true });
    }

    #[test]
    fn the_address_follows_the_latest_request() {
        let mut v = Viewers::new();
        let now = Instant::now();
        v.watch(1, [1; 32], addr(1), true, now);
        v.watch(1, [1; 32], addr(2), true, now);
        assert_eq!(v.addrs(), vec![addr(2)]);
    }

    #[test]
    fn worst_loss_counts_only_current_viewers_and_recent_reports() {
        let mut v = Viewers::new();
        let t0 = Instant::now();
        v.watch(1, [1; 32], addr(1), true, t0);
        v.watch(2, [2; 32], addr(2), true, t0);
        v.report_loss(1, 10, t0);
        v.report_loss(2, 150, t0);
        v.report_loss(3, 999, t0); // not watching
        assert_eq!(v.worst_loss(t0), Some(150));
        v.unwatch(2);
        assert_eq!(v.worst_loss(t0), Some(10));
        assert_eq!(v.worst_loss(t0 + LOSS_REPORT_AGE), None);
    }

    #[test]
    fn rate_cuts_on_heavy_loss_at_most_once_a_second_and_never_below_the_floor() {
        let mut r = RateControl::new(8_000_000);
        let t0 = Instant::now();
        assert_eq!(r.update(Some(200), t0), 6_400_000);
        assert_eq!(r.update(Some(200), t0 + Duration::from_millis(500)), 6_400_000);
        assert_eq!(r.update(Some(200), t0 + Duration::from_secs(1)), 5_120_000);
        let mut t = t0 + Duration::from_secs(1);
        for _ in 0..20 {
            t += Duration::from_secs(1);
            r.update(Some(500), t);
        }
        assert_eq!(r.current(), MIN_BITRATE);
    }

    #[test]
    fn rate_climbs_back_after_five_clean_seconds_up_to_the_configured_value() {
        let mut r = RateControl::new(8_000_000);
        let t0 = Instant::now();
        r.update(Some(200), t0);
        assert_eq!(r.current(), 6_400_000);
        let mut t = t0;
        let mut steps = vec![];
        for _ in 0..60 {
            t += Duration::from_secs(1);
            let before = r.current();
            if r.update(Some(5), t) != before {
                steps.push(t.duration_since(t0).as_secs());
            }
        }
        assert_eq!(steps, vec![6, 11, 16, 21]);
        assert_eq!(r.current(), 8_000_000);
    }

    #[test]
    fn moderate_loss_or_silence_holds_the_rate() {
        let mut r = RateControl::new(8_000_000);
        let t0 = Instant::now();
        r.update(Some(200), t0);
        let mut t = t0;
        for i in 0..30 {
            t += Duration::from_secs(1);
            // Clean, but interrupted by moderate loss every 4 s: never 5 clean seconds.
            r.update(if i % 4 == 3 { Some(50) } else { Some(0) }, t);
        }
        assert_eq!(r.current(), 6_400_000);
        r.update(None, t + Duration::from_secs(60));
        assert_eq!(r.current(), 6_400_000);
    }
}
