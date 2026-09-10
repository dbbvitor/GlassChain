//! Shared real-TCP fault proxy for the network integration tests (#70, D7).
//!
//! Nodes run over **real loopback TCP** with an in-process proxy between
//! ordered pairs: the dialer connects to the proxy front port, the proxy
//! relays raw byte streams to the target node, and `partition` aborts the
//! established relay tasks and refuses new connections until `repair`.
//!
//! The WAN extension (performance plan §5, D7): a seedable one-way
//! latency/jitter profile and a bandwidth budget applied per direction.
//! Boundedness comes from the relay loop's own backpressure — shaping runs
//! in-line before the write, so in-flight bytes are bounded by the socket
//! buffers and no unbounded queue can grow.
//!
//! These are **real TCP wall-clock tests**, not deterministic simulated
//! network runs; margins are generous and results are labeled separately
//! from madsim-style deterministic execution.

use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
    task::JoinHandle,
};

/// Seedable one-way WAN profile: base latency ± jitter (ms) and a byte
/// budget per direction per second (0 = unbounded).
#[derive(Clone, Copy, Debug)]
pub struct WanProfile {
    pub latency_ms: u64,
    pub jitter_ms: u64,
    pub bandwidth_bps: u64,
}

impl WanProfile {
    /// No shaping — the no-fault baseline.
    pub const fn none() -> Self {
        Self {
            latency_ms: 0,
            jitter_ms: 0,
            bandwidth_bps: 0,
        }
    }
}

/// Deterministic xorshift64 so scenarios are repeatable across runs.
struct Seeded(u64);

impl Seeded {
    const fn new(seed: u64) -> Self {
        // Zero seeds would collapse the xorshift state; the tests never
        // pass one, but a saturated start costs nothing.
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    const fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// One-way delay for this chunk: `latency + jitter` (uniform in
    /// `[0, jitter]`), plus bandwidth pacing (`bytes * 1000 / bps` ms).
    fn shaped_delay(&mut self, profile: &WanProfile, bytes: usize) -> Duration {
        let jitter = if profile.jitter_ms == 0 {
            0
        } else {
            self.next() % profile.jitter_ms.max(1)
        };
        let mut ms = profile.latency_ms.saturating_add(jitter);
        if profile.bandwidth_bps > 0 {
            #[allow(clippy::cast_sign_loss)]
            let paced = (bytes as u64 * 1_000) / profile.bandwidth_bps.max(1);
            ms += paced.max(1);
        }
        Duration::from_millis(ms)
    }
}

// Shared module: each test crate compiles this file directly, so members one
// crate does not use show up as dead code there — the allow is per-module, not
// per-call-site.
#[allow(dead_code)]
/// A bidirectional WAN-shaped relay between one dialer-side front port and
/// one target address. `partition` aborts the relay tasks (severing the
/// established sockets) and refuses new connections; `repair` allows them
/// again; `set_profile` switches the shaping mid-scenario.
pub struct TcpProxy {
    /// Front port — the address dialers use.
    front_addr: String,
    enabled: Arc<std::sync::atomic::AtomicBool>,
    relays: Arc<Mutex<Vec<JoinHandle<()>>>>,
    profile: Arc<Mutex<WanProfile>>,
}

#[allow(dead_code)]
impl TcpProxy {
    /// Proxy a freshly bound front port to `target` with no shaping.
    pub async fn spawn(target: &str) -> Self {
        Self::spawn_with_profile(target, WanProfile::none()).await
    }

    /// Proxy a freshly bound front port to `target`, shaping every relayed
    /// byte with `profile`.
    pub async fn spawn_with_profile(target: &str, profile: WanProfile) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let front_addr = listener.local_addr().unwrap().to_string();
        let enabled = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let relays: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::default();
        let profile: Arc<Mutex<WanProfile>> = Arc::new(Mutex::new(profile));

        let acceptor_enabled = Arc::clone(&enabled);
        let acceptor_relays = Arc::clone(&relays);
        let acceptor_profile = Arc::clone(&profile);
        let target = target.to_owned();
        tokio::spawn(async move {
            loop {
                let Ok((client, _)) = listener.accept().await else {
                    break;
                };
                if !acceptor_enabled.load(Ordering::SeqCst) {
                    // Partition: refuse the connection outright.
                    drop(client);
                    continue;
                }
                let Ok(upstream) = TcpStream::connect(&target).await else {
                    drop(client);
                    continue;
                };
                let relay_profile = Arc::clone(&acceptor_profile);
                acceptor_relays.lock().await.push(tokio::spawn(async move {
                    // One shaped task per direction; aborting both (partition)
                    // drops both sockets and the TCP stacks see the disconnect.
                    let (client_read, client_write) = client.into_split();
                    let (upstream_read, upstream_write) = upstream.into_split();
                    let down = shaped_relay(
                        client_read,
                        upstream_write,
                        Arc::clone(&relay_profile),
                        0x1234_5678_9abc_def0,
                    );
                    let up = shaped_relay(
                        upstream_read,
                        client_write,
                        Arc::clone(&relay_profile),
                        0xfeed_face_dead_beef,
                    );
                    let _ = tokio::join!(down, up);
                }));
            }
        });

        Self {
            front_addr,
            enabled,
            relays,
            profile,
        }
    }

    /// The front address dialers must use for this proxy.
    pub fn front_addr(&self) -> &str {
        &self.front_addr
    }

    /// Switch the WAN shaping applied to every relay from now on. New bytes
    /// use the new profile; bytes already in flight are not re-shaped.
    pub async fn set_profile(&self, profile: WanProfile) {
        *self.profile.lock().await = profile;
    }

    /// Sever every established relay and refuse new connections.
    pub async fn partition(&self) {
        self.enabled.store(false, Ordering::SeqCst);
        let mut relays = self.relays.lock().await;
        for handle in relays.drain(..) {
            handle.abort();
        }
    }

    /// Allow new connections again.
    pub fn repair(&self) {
        self.enabled.store(true, Ordering::SeqCst);
    }
}

async fn shaped_relay(
    mut from: tokio::net::tcp::OwnedReadHalf,
    mut to: tokio::net::tcp::OwnedWriteHalf,
    profile: Arc<Mutex<WanProfile>>,
    seed: u64,
) {
    let mut rng = Seeded::new(seed);
    let mut buf = vec![0u8; 4_096];
    loop {
        match from.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(len) => {
                let shaping = *profile.lock().await;
                let delay = rng.shaped_delay(&shaping, len);
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                if to.write_all(&buf[..len]).await.is_err() {
                    break;
                }
            }
        }
    }
}
