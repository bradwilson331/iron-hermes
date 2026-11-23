//! Supervised client for the `ironhermes-tirith` sidecar (Phase 57, D-02 / D-12).
//!
//! One long-lived child per [`SidecarClient`]. Requests are id-tagged JSON lines
//! written by a dedicated writer thread; a dedicated reader thread parses replies
//! and routes each one to the responder registered under its id. There is no spawn
//! per check, and the client never hops into or blocks on a tokio runtime (the web
//! per-connection `LocalSet` forbids that): the sync entry point waits on a plain
//! `std::sync::mpsc` channel and the async one on a tokio oneshot, so both work
//! from a plain thread and from a `LocalSet`.
//!
//! Every failure is a typed [`Unavailable`]; nothing here ever turns an error into
//! an empty (clean) finding list (D-12).
//!
//! Lock order: `state` may be held while taking `pending`; nothing takes `state`
//! while holding `pending`.

use std::collections::HashMap;
use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ironhermes_security_proto::{
    ERR_TOO_LARGE, ERR_UNSUPPORTED_SUBJECT, MAX_SUBJECT_BYTES, Op, Request, Response,
    SidecarVersion, SubjectKind, WireFinding,
};
use tokio::sync::oneshot;

use crate::locate::locate_sidecar_binary;

/// Slack allowed on top of the subject size for the JSON envelope; mirrors the
/// sidecar's own `MAX_SUBJECT_BYTES + 4096` line cap.
const LINE_SLACK: usize = 4096;

/// Why the sidecar could not give a verdict. Exactly one per failure (D-12).
///
/// Two classes, told apart by [`UnavailableReason::is_outage`]:
///
/// - an **outage** (`Missing`, `Crash`): the sidecar is down, whatever the subject;
/// - every other reason: the sidecar is up but **this subject was not screened**
///   (too large, refused, malformed reply, too slow, or the child was killed
///   after repeated timeouts). The text's author can cause these, so policy
///   fails closed on them (CR-01).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnavailableReason {
    /// No sidecar binary could be located or executed.
    Missing,
    /// The child exited on its own, a write to it failed, or the client is backing
    /// off after that.
    Crash,
    /// The child did not answer within the request budget.
    Timeout,
    /// The child was killed because it kept missing its budget
    /// (`timeout_kill_threshold` consecutive timeouts), and every request until the
    /// next spawn fails with this reason. The author of a subject that hangs the
    /// analysis can cause it, so it is a not-screened reason, not an outage.
    KilledForTimeouts,
    /// The subject exceeds the size cap and was never sent.
    TooLarge,
    /// The child answered, but with an error or something unparseable.
    ProtocolError,
    /// The sidecar refused this subject (a refused or unsafe file name, a PDF
    /// header, a NUL byte, or a non-text classification). Caused by the subject,
    /// not by the sidecar's health.
    Unsupported,
}

impl UnavailableReason {
    /// True when the sidecar itself is down (`Missing`, `Crash`). False when the
    /// sidecar is up but did not screen this subject.
    pub fn is_outage(self) -> bool {
        matches!(self, Self::Missing | Self::Crash)
    }

    /// Every reason, in declaration order.
    pub const ALL: [UnavailableReason; 7] = [
        Self::Missing,
        Self::Crash,
        Self::Timeout,
        Self::KilledForTimeouts,
        Self::TooLarge,
        Self::ProtocolError,
        Self::Unsupported,
    ];

    /// The exact inverse of the `Display` word (the wire and audit spelling).
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.to_string() == word)
    }
}

impl fmt::Display for UnavailableReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Missing => "missing",
            Self::Crash => "crash",
            Self::Timeout => "timeout",
            Self::KilledForTimeouts => "killed_for_timeouts",
            Self::TooLarge => "too_large",
            Self::ProtocolError => "protocol_error",
            Self::Unsupported => ERR_UNSUPPORTED_SUBJECT,
        })
    }
}

/// A typed "no verdict" error. Callers must map it to Warn/Block by policy; it is
/// never equivalent to "no findings".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable {
    pub reason: UnavailableReason,
    pub detail: String,
}

impl Unavailable {
    fn new(reason: UnavailableReason, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sidecar unavailable ({}): {}", self.reason, self.detail)
    }
}

impl std::error::Error for Unavailable {}

/// Construction options. Defaults come from [`SidecarOptions::for_home`].
#[derive(Debug, Clone)]
pub struct SidecarOptions {
    /// Sidecar binary; `None` means [`locate_sidecar_binary`].
    pub binary: Option<PathBuf>,
    /// Dedicated, empty `HOME` / `XDG_*` directory for the child.
    pub home_dir: PathBuf,
    /// Dedicated `TIRITH_POLICY_ROOT` for the child.
    pub policy_root: PathBuf,
    /// Budget for a request on a warm child.
    pub steady_timeout: Duration,
    /// Budget for the first check after a spawn (cold start).
    pub first_timeout: Duration,
    /// Extra budget per KiB of work on a warm child: the check's own subject plus
    /// the check subjects already queued ahead of it on the same child. The child
    /// analyses requests in order, and tirith's time is superlinear in
    /// command-dense text.
    pub budget_per_kib: Duration,
    /// Ceiling on the scaled warm budget (never below `steady_timeout`).
    pub budget_ceiling: Duration,
    /// Largest subject accepted before anything is written.
    pub max_subject_bytes: usize,
    /// First backoff after a crash, and the flat backoff after the child is killed
    /// for repeated timeouts (that one never doubles).
    pub backoff_min: Duration,
    /// Backoff ceiling.
    pub backoff_max: Duration,
    /// A child healthy for this long resets the backoff.
    pub healthy_reset: Duration,
    /// Consecutive timeouts after which the child is killed. Every request until
    /// the next spawn then fails with `KilledForTimeouts`, for a flat `backoff_min`.
    pub timeout_kill_threshold: u32,
    /// Extra argv for the child. Empty in production; tests pass `--mode ..`.
    pub extra_args: Vec<String>,
}

impl SidecarOptions {
    /// Production defaults rooted at a hermes home: the child's HOME is
    /// `<home>/security/tirith-home` and its policy root `<home>/security/tirith-policy`
    /// (both created 0700).
    pub fn for_home(hermes_home: &Path) -> Self {
        let home_dir = hermes_home.join("security").join("tirith-home");
        let policy_root = hermes_home.join("security").join("tirith-policy");
        ensure_private_dir(&home_dir);
        ensure_private_dir(&policy_root);
        Self {
            binary: None,
            home_dir,
            policy_root,
            steady_timeout: Duration::from_millis(250),
            first_timeout: Duration::from_secs(3),
            budget_per_kib: Duration::from_millis(10),
            budget_ceiling: Duration::from_secs(3),
            max_subject_bytes: MAX_SUBJECT_BYTES,
            backoff_min: Duration::from_millis(500),
            backoff_max: Duration::from_secs(30),
            healthy_reset: Duration::from_secs(60),
            timeout_kill_threshold: 3,
            extra_args: Vec::new(),
        }
    }

    /// [`SidecarOptions::for_home`] at the process's hermes home.
    pub fn for_default_home() -> Self {
        Self::for_home(&ironhermes_core::get_hermes_home())
    }
}

fn ensure_private_dir(dir: &Path) {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    if let Err(e) = builder.create(dir) {
        tracing::warn!(dir = %dir.display(), error = %e, "could not create sidecar scratch dir");
    }
}

/// Coarse supervision state, for health reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarState {
    /// Running and has answered at least once.
    Healthy,
    /// No usable binary.
    Missing,
    /// Crashed; refusing requests until the backoff elapses.
    Backoff,
    /// Spawned, no answer yet.
    Starting,
}

/// Snapshot returned by [`SidecarClient::health`].
#[derive(Debug, Clone)]
pub struct SidecarHealth {
    pub state: SidecarState,
    pub respawns: u32,
    pub consecutive_timeouts: u32,
    pub late_replies: u64,
    pub last_unavailable: Option<UnavailableReason>,
    pub binary: Option<PathBuf>,
}

/// What a waiter receives for its request id.
enum Delivery {
    Response(Response),
    Failed(Unavailable),
}

enum Responder {
    Async(oneshot::Sender<Delivery>),
    Sync(SyncSender<Delivery>),
    /// Fire-and-forget (the warm-up ping): the answer is only used to confirm the
    /// child is alive.
    Warmup,
    /// A request whose waiter gave up (its budget expired) while the child was
    /// still working on it. The entry is kept so its bytes still count toward the
    /// queue accounting and its late reply is recognised, counted and dropped.
    Abandoned,
}

impl Responder {
    fn deliver(self, delivery: Delivery) {
        match self {
            // A dropped receiver (timed out) is exactly the late-reply case: ignore.
            Self::Async(tx) => {
                let _ = tx.send(delivery);
            }
            Self::Sync(tx) => {
                let _ = tx.try_send(delivery);
            }
            Self::Warmup | Self::Abandoned => {}
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PendKind {
    /// A `check`; its answer marks the child warm.
    Check,
    Other,
}

struct Pending {
    epoch: u64,
    kind: PendKind,
    /// Subject length for a check; 0 for ping and version.
    bytes: usize,
    responder: Responder,
}

struct ChildSlot {
    epoch: u64,
    child: Child,
    tx: mpsc::Sender<String>,
    spawned_at: Instant,
    /// Any reply seen from this child.
    confirmed: bool,
    /// A `check` reply seen from this child; until then checks get the longer
    /// first-request budget.
    warm: bool,
}

enum Phase {
    Missing,
    /// Not running until `until`. `reason` is why, and is what every request in the
    /// window fails with: `Crash` for a child that exited on its own,
    /// `KilledForTimeouts` for one this client killed.
    Backoff {
        until: Instant,
        reason: UnavailableReason,
    },
    Running,
}

struct StateData {
    phase: Phase,
    slot: Option<ChildSlot>,
    next_epoch: u64,
    /// Backoff applied after the most recent crash; `None` until the first one.
    backoff_prev: Option<Duration>,
    respawns: u32,
    consecutive_timeouts: u32,
    last_unavailable: Option<UnavailableReason>,
    /// The epoch of the most recent child to exit, and why. Lets a request that
    /// races the exit (or a thread that only notices the death afterwards) report
    /// the real reason instead of a hard-coded `Crash`.
    last_exit: Option<(u64, UnavailableReason)>,
    shutdown: bool,
}

struct Shared {
    opts: SidecarOptions,
    binary: Option<PathBuf>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, Pending>>,
    state: Mutex<StateData>,
    late_replies: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The time a request may take. A cold child gets `first_timeout`. A warm child gets
/// `steady_timeout` plus `budget_per_kib` for every KiB of its own subject and of the
/// check subjects queued ahead of it, capped at `budget_ceiling` (never below
/// `steady_timeout`).
fn request_budget(
    opts: &SidecarOptions,
    warm: bool,
    subject_bytes: usize,
    queued_bytes: usize,
) -> Duration {
    if !warm {
        return opts.first_timeout;
    }
    let kib = subject_bytes.saturating_add(queued_bytes) / 1024;
    let scaled = opts.steady_timeout.saturating_add(
        opts.budget_per_kib
            .saturating_mul(u32::try_from(kib).unwrap_or(u32::MAX)),
    );
    scaled.min(opts.budget_ceiling.max(opts.steady_timeout))
}

/// What `submit` hands back to the waiting side.
struct Ticket {
    id: u64,
    epoch: u64,
    budget: Duration,
}

impl Shared {
    /// Spawn the child, its reader and writer threads, and queue the warm-up ping.
    /// Called with `state` locked; on failure it sets the phase accordingly.
    fn start_child(self: &Arc<Self>, st: &mut StateData) -> Result<(), Unavailable> {
        let Some(binary) = self.binary.as_ref() else {
            st.phase = Phase::Missing;
            return Err(Unavailable::new(
                UnavailableReason::Missing,
                "sidecar binary not found",
            ));
        };
        ensure_private_dir(&self.opts.home_dir);
        ensure_private_dir(&self.opts.policy_root);

        let mut cmd = Command::new(binary);
        cmd.args(&self.opts.extra_args)
            // Pitfall 7: nothing ambient reaches the child. No TIRITH,
            // TIRITH_SERVER_URL, TIRITH_API_KEY and not the operator's real HOME.
            .env_clear()
            .env("HOME", &self.opts.home_dir)
            .env("XDG_CONFIG_HOME", &self.opts.home_dir)
            .env("XDG_DATA_HOME", &self.opts.home_dir)
            .env("TIRITH_LOG", "0")
            .env("TIRITH_POLICY_ROOT", &self.opts.policy_root)
            .env("PATH", "/usr/bin:/bin")
            .current_dir(&self.opts.home_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let missing = matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                );
                let reason = if missing {
                    UnavailableReason::Missing
                } else {
                    UnavailableReason::Crash
                };
                if missing {
                    st.phase = Phase::Missing;
                } else {
                    self.schedule_backoff(st, None, UnavailableReason::Crash);
                }
                return Err(Unavailable::new(
                    reason,
                    format!("could not spawn {}: {e}", binary.display()),
                ));
            }
        };
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            self.schedule_backoff(st, None, UnavailableReason::Crash);
            return Err(Unavailable::new(
                UnavailableReason::Crash,
                "child stdio was not piped",
            ));
        };

        let epoch = st.next_epoch;
        st.next_epoch += 1;
        let (tx, rx) = mpsc::channel::<String>();

        let reader_shared = Arc::clone(self);
        let writer_shared = Arc::clone(self);
        let threads = std::thread::Builder::new()
            .name("tirith-sidecar-reader".into())
            .spawn(move || reader_loop(reader_shared, epoch, stdout))
            .and_then(|_| {
                std::thread::Builder::new()
                    .name("tirith-sidecar-writer".into())
                    .spawn(move || writer_loop(writer_shared, epoch, stdin, rx))
            });
        if let Err(e) = threads {
            // Closing the child's pipes lets any thread that did start exit.
            let _ = child.kill();
            let _ = child.wait();
            self.schedule_backoff(st, None, UnavailableReason::Crash);
            return Err(Unavailable::new(
                UnavailableReason::Crash,
                format!("could not start sidecar io threads: {e}"),
            ));
        }
        st.consecutive_timeouts = 0;

        st.slot = Some(ChildSlot {
            epoch,
            child,
            tx,
            spawned_at: Instant::now(),
            confirmed: false,
            warm: false,
        });
        st.phase = Phase::Running;

        // Warm-up ping: confirms the child is alive without waiting for a real
        // subject. Pipe ordering means it is answered before any later request.
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let ping = Request {
            id,
            op: Op::Ping,
            kind: None,
            text: None,
            name: None,
        };
        if let Ok(mut line) = serde_json::to_string(&ping) {
            line.push('\n');
            lock(&self.pending).insert(
                id,
                Pending {
                    epoch,
                    kind: PendKind::Other,
                    bytes: 0,
                    responder: Responder::Warmup,
                },
            );
            if let Some(slot) = st.slot.as_ref() {
                let _ = slot.tx.send(line);
            }
        }
        Ok(())
    }

    /// Record the backoff for an exit that just happened: the first crash waits
    /// `backoff_min`, later ones double up to `backoff_max`, and a child that had
    /// been healthy for `healthy_reset` starts over at the minimum.
    ///
    /// A kill for timeouts is the exception: it is evidence of a hanging subject,
    /// not of a broken binary, and every request in its window fails closed, so a
    /// long window would be an author-triggered denial of service for every
    /// session. It waits `backoff_min` every time and leaves the crash doubling
    /// untouched. A genuinely hung binary stays bounded, because each cycle needs
    /// `timeout_kill_threshold` timeouts, the first at the cold budget.
    fn schedule_backoff(
        &self,
        st: &mut StateData,
        spawned_at: Option<Instant>,
        reason: UnavailableReason,
    ) {
        let opts = &self.opts;
        let delay = if reason == UnavailableReason::KilledForTimeouts {
            opts.backoff_min
        } else {
            let delay = match st.backoff_prev {
                None => opts.backoff_min,
                Some(prev) => backoff_after_healthy(
                    spawned_at.map_or(Duration::ZERO, |t| t.elapsed()),
                    opts.healthy_reset,
                    prev,
                    opts.backoff_min,
                    opts.backoff_max,
                ),
            };
            st.backoff_prev = Some(delay);
            delay
        };
        st.phase = Phase::Backoff {
            until: Instant::now() + delay,
            reason,
        };
    }

    /// Why the child of `epoch` is gone: the reason recorded when it exited, else
    /// `Crash`.
    fn exit_reason(&self, epoch: u64) -> UnavailableReason {
        match lock(&self.state).last_exit {
            Some((e, reason)) if e == epoch => reason,
            _ => UnavailableReason::Crash,
        }
    }

    /// Pick the child to talk to, lazily respawning once a backoff has elapsed.
    /// While backing off a request fails immediately instead of queueing.
    fn acquire(self: &Arc<Self>) -> Result<(mpsc::Sender<String>, u64, bool), Unavailable> {
        let mut st = lock(&self.state);
        if st.shutdown {
            return Err(Unavailable::new(
                UnavailableReason::Crash,
                "client is shutting down",
            ));
        }
        if let Phase::Backoff { until, reason } = st.phase {
            let now = Instant::now();
            if now < until {
                let remaining = until.saturating_duration_since(now);
                let detail = if reason == UnavailableReason::KilledForTimeouts {
                    format!(
                        "sidecar was killed after repeated timeouts; restarting in {remaining:?}"
                    )
                } else {
                    format!("sidecar exited; backing off for another {remaining:?}")
                };
                return Err(Unavailable::new(reason, detail));
            }
            st.respawns += 1;
            tracing::info!(respawns = st.respawns, "respawning tirith sidecar");
            self.start_child(&mut st)?;
        }
        match st.phase {
            Phase::Running => match st.slot.as_ref() {
                Some(slot) => Ok((slot.tx.clone(), slot.epoch, slot.warm)),
                None => Err(Unavailable::new(UnavailableReason::Crash, "no child")),
            },
            // `start_child` leaves `Missing` or a fresh backoff on failure; it
            // cannot leave a backoff behind on success.
            Phase::Missing => Err(Unavailable::new(
                UnavailableReason::Missing,
                "sidecar binary not found",
            )),
            Phase::Backoff { reason, .. } => {
                Err(Unavailable::new(reason, "sidecar is not running"))
            }
        }
    }

    fn is_live(&self, epoch: u64) -> bool {
        lock(&self.state)
            .slot
            .as_ref()
            .is_some_and(|slot| slot.epoch == epoch)
    }

    /// Register a request and write its line. Returns before waiting.
    fn submit(
        self: &Arc<Self>,
        req_fields: (Op, Option<SubjectKind>, Option<&str>, Option<&str>),
        kind: PendKind,
        responder: Responder,
    ) -> Result<Ticket, Unavailable> {
        let (op, subject_kind, text, name) = req_fields;
        if op == Op::Check && text.is_some_and(|t| t.len() > self.opts.max_subject_bytes) {
            return Err(Unavailable::new(
                UnavailableReason::TooLarge,
                format!("subject exceeds {} bytes", self.opts.max_subject_bytes),
            ));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let req = Request {
            id,
            op,
            kind: subject_kind,
            text: text.map(str::to_string),
            name: name.map(str::to_string),
        };
        let mut line = serde_json::to_string(&req)
            .map_err(|e| Unavailable::new(UnavailableReason::ProtocolError, e.to_string()))?;
        if line.len() > self.opts.max_subject_bytes + LINE_SLACK {
            // JSON escaping can push a within-limit subject over the sidecar's
            // line cap; the sidecar would answer with an id-less error.
            return Err(Unavailable::new(
                UnavailableReason::TooLarge,
                "encoded request exceeds the line cap",
            ));
        }
        line.push('\n');

        let (tx, epoch, warm) = self.acquire()?;
        let text_len = if kind == PendKind::Check {
            text.map_or(0, str::len)
        } else {
            0
        };
        // The child analyses requests in order, so a request waits for the check
        // subjects already queued on this child as well as its own analysis. A
        // check whose budget expired still counts: the child is still analysing it
        // (`Responder::Abandoned`) until its late reply arrives or its child exits.
        let queued = {
            let mut pending = lock(&self.pending);
            let queued = pending
                .values()
                .filter(|p| p.epoch == epoch && p.kind == PendKind::Check)
                .map(|p| p.bytes)
                .sum::<usize>();
            pending.insert(
                id,
                Pending {
                    epoch,
                    kind,
                    bytes: text_len,
                    responder,
                },
            );
            queued
        };
        let budget = request_budget(&self.opts, warm, text_len, queued);
        // The child may have died between `acquire` and the insert, after its
        // pending entries were already drained: re-check so the request is failed
        // now instead of waiting out its budget.
        if !self.is_live(epoch) {
            lock(&self.pending).remove(&id);
            return Err(Unavailable::new(
                self.exit_reason(epoch),
                "sidecar exited before the request was sent",
            ));
        }
        if tx.send(line).is_err() {
            lock(&self.pending).remove(&id);
            self.on_child_exit(epoch, "writer thread is gone", UnavailableReason::Crash);
            return Err(Unavailable::new(
                self.exit_reason(epoch),
                "sidecar writer is gone",
            ));
        }
        Ok(Ticket { id, epoch, budget })
    }

    /// The child for `epoch` is gone (EOF, write error, or killed): take its slot,
    /// reap it, enter backoff carrying `reason`, and fail every request that was
    /// waiting on it. When another thread already took the slot, the reason it
    /// recorded wins, so a reader or writer thread that only notices the death after
    /// a kill cannot relabel it `Crash`.
    fn on_child_exit(self: &Arc<Self>, epoch: u64, why: &str, reason: UnavailableReason) {
        let (slot, fail_reason) = {
            let mut st = lock(&self.state);
            let taken = st.slot.take_if(|s| s.epoch == epoch);
            let fail_reason = if let Some(dead) = taken.as_ref()
                && !st.shutdown
            {
                self.schedule_backoff(&mut st, Some(dead.spawned_at), reason);
                st.consecutive_timeouts = 0;
                st.last_unavailable = Some(reason);
                st.last_exit = Some((epoch, reason));
                reason
            } else if taken.is_some() {
                reason
            } else {
                match st.last_exit {
                    Some((e, recorded)) if e == epoch => recorded,
                    _ => UnavailableReason::Crash,
                }
            };
            (taken, fail_reason)
        };
        if let Some(mut slot) = slot {
            let _ = slot.child.kill();
            let _ = slot.child.wait();
            tracing::warn!(reason = why, "tirith sidecar exited");
        }
        self.fail_epoch(epoch, fail_reason, why);
    }

    /// Fail every request still waiting on the child of `epoch`.
    fn fail_epoch(&self, epoch: u64, reason: UnavailableReason, detail: &str) {
        let drained: Vec<Pending> = {
            let mut pending = lock(&self.pending);
            let ids: Vec<u64> = pending
                .iter()
                .filter(|(_, p)| p.epoch == epoch)
                .map(|(id, _)| *id)
                .collect();
            ids.into_iter()
                .filter_map(|id| pending.remove(&id))
                .collect()
        };
        for p in drained {
            p.responder
                .deliver(Delivery::Failed(Unavailable::new(reason, detail)));
        }
    }

    fn handle_reply_line(self: &Arc<Self>, epoch: u64, line: &str) {
        let Ok(resp) = serde_json::from_str::<Response>(line) else {
            // A reply with no usable id cannot be matched to a request, so every
            // request waiting on this child gets a typed protocol error now rather
            // than a timeout later. The child itself is kept: a desynchronised
            // stream still yields typed errors, never a clean verdict.
            tracing::warn!("unparseable line from tirith sidecar");
            self.fail_epoch(
                epoch,
                UnavailableReason::ProtocolError,
                "unparseable reply from sidecar",
            );
            return;
        };
        let pending = lock(&self.pending).remove(&resp.id);
        let Some(pending) = pending else {
            // Its entry is gone (it never existed, or its child was failed): drop
            // it. It must never be handed to a later request (Pitfall 5).
            if resp.id != 0 {
                self.late_replies.fetch_add(1, Ordering::Relaxed);
            }
            tracing::debug!(id = resp.id, "dropped late or unknown sidecar reply");
            return;
        };
        if matches!(pending.responder, Responder::Abandoned) {
            // The late reply to a request that timed out: counted and dropped. It
            // neither marks the child warm nor resets the timeout streak, so the
            // kill rule is unchanged.
            self.late_replies.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(id = resp.id, "dropped late sidecar reply");
            return;
        }
        {
            let mut st = lock(&self.state);
            if let Some(slot) = st.slot.as_mut().filter(|s| s.epoch == epoch) {
                slot.confirmed = true;
                if pending.kind == PendKind::Check {
                    slot.warm = true;
                }
                st.consecutive_timeouts = 0;
            }
        }
        pending.responder.deliver(Delivery::Response(resp));
    }

    /// A request timed out. Its id is not forgotten: the entry becomes
    /// `Responder::Abandoned`, so its bytes still count toward the queue accounting
    /// while the child is still on it. A late reply is still never handed to a later
    /// request, because entries are keyed by id and an abandoned entry delivers
    /// nothing (Pitfall 5). Then count the timeout and kill a child that keeps
    /// missing its budget (it is hung, not just slow).
    fn on_timeout(self: &Arc<Self>, id: u64, epoch: u64) {
        if let Some(p) = lock(&self.pending).get_mut(&id) {
            p.responder = Responder::Abandoned;
        }
        let kill = {
            let mut st = lock(&self.state);
            if st.slot.as_ref().is_some_and(|s| s.epoch == epoch) {
                st.consecutive_timeouts += 1;
                st.consecutive_timeouts >= self.opts.timeout_kill_threshold
            } else {
                false
            }
        };
        if kill {
            tracing::warn!(
                threshold = self.opts.timeout_kill_threshold,
                "killing tirith sidecar after repeated timeouts"
            );
            self.on_child_exit(
                epoch,
                "killed after repeated timeouts",
                UnavailableReason::KilledForTimeouts,
            );
        }
    }

    fn record<T>(&self, result: Result<T, Unavailable>) -> Result<T, Unavailable> {
        if let Err(e) = &result {
            lock(&self.state).last_unavailable = Some(e.reason);
        }
        result
    }

    fn shutdown(self: &Arc<Self>) {
        let slot = {
            let mut st = lock(&self.state);
            st.shutdown = true;
            st.slot.take()
        };
        if let Some(mut slot) = slot {
            let _ = slot.child.kill();
            let _ = slot.child.wait();
        }
        let drained: Vec<Pending> = lock(&self.pending).drain().map(|(_, p)| p).collect();
        for p in drained {
            p.responder.deliver(Delivery::Failed(Unavailable::new(
                UnavailableReason::Crash,
                "client dropped",
            )));
        }
    }
}

fn reader_loop(shared: Arc<Shared>, epoch: u64, stdout: ChildStdout) {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => shared.handle_reply_line(epoch, line.trim_end()),
        }
    }
    shared.on_child_exit(epoch, "sidecar closed its output", UnavailableReason::Crash);
}

fn writer_loop(shared: Arc<Shared>, epoch: u64, mut stdin: ChildStdin, rx: mpsc::Receiver<String>) {
    while let Ok(line) = rx.recv() {
        if stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .is_err()
        {
            shared.on_child_exit(epoch, "write to sidecar failed", UnavailableReason::Crash);
            return;
        }
    }
}

fn interpret_check(delivery: Delivery) -> Result<Vec<WireFinding>, Unavailable> {
    match delivery {
        Delivery::Failed(e) => Err(e),
        Delivery::Response(resp) if resp.ok => Ok(resp.findings),
        Delivery::Response(resp) => {
            let err = resp.err.unwrap_or_else(|| "unspecified error".to_string());
            if err == ERR_TOO_LARGE {
                Err(Unavailable::new(UnavailableReason::TooLarge, err))
            } else if err == ERR_UNSUPPORTED_SUBJECT {
                Err(Unavailable::new(UnavailableReason::Unsupported, err))
            } else {
                Err(Unavailable::new(UnavailableReason::ProtocolError, err))
            }
        }
    }
}

fn interpret_ok(delivery: Delivery) -> Result<Response, Unavailable> {
    match delivery {
        Delivery::Failed(e) => Err(e),
        Delivery::Response(resp) if resp.ok => Ok(resp),
        Delivery::Response(resp) => Err(Unavailable::new(
            UnavailableReason::ProtocolError,
            resp.err.unwrap_or_else(|| "unspecified error".to_string()),
        )),
    }
}

struct Guard {
    shared: Arc<Shared>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.shared.shutdown();
    }
}

/// Cheaply cloneable handle to one supervised sidecar child.
#[derive(Clone)]
pub struct SidecarClient {
    guard: Arc<Guard>,
}

impl SidecarClient {
    /// Start supervising a sidecar. Never fails: a missing binary yields a client
    /// whose every request is `Unavailable { Missing }`.
    pub fn spawn(opts: SidecarOptions) -> Self {
        let binary = opts
            .binary
            .clone()
            .or_else(locate_sidecar_binary)
            .filter(|p| p.is_file());
        let shared = Arc::new(Shared {
            opts,
            binary,
            next_id: AtomicU64::new(0),
            pending: Mutex::new(HashMap::new()),
            state: Mutex::new(StateData {
                phase: Phase::Missing,
                slot: None,
                next_epoch: 1,
                backoff_prev: None,
                respawns: 0,
                consecutive_timeouts: 0,
                last_unavailable: None,
                last_exit: None,
                shutdown: false,
            }),
            late_replies: AtomicU64::new(0),
        });
        {
            let mut st = lock(&shared.state);
            if let Err(e) = shared.start_child(&mut st) {
                st.last_unavailable = Some(e.reason);
                tracing::warn!(error = %e, "tirith sidecar did not start");
            }
        }
        Self {
            guard: Arc::new(Guard { shared }),
        }
    }

    fn shared(&self) -> &Arc<Shared> {
        &self.guard.shared
    }

    /// Screen a subject. `Ok` means the sidecar answered (possibly with no
    /// findings); every failure is an `Err`.
    pub async fn check(
        &self,
        kind: SubjectKind,
        text: &str,
        name: Option<&str>,
    ) -> Result<Vec<WireFinding>, Unavailable> {
        let shared = self.shared();
        let (tx, rx) = oneshot::channel();
        let result = async {
            let ticket = shared.submit(
                (Op::Check, Some(kind), Some(text), name),
                PendKind::Check,
                Responder::Async(tx),
            )?;
            match tokio::time::timeout(ticket.budget, rx).await {
                Ok(Ok(delivery)) => interpret_check(delivery),
                Ok(Err(_closed)) => Err(Unavailable::new(
                    UnavailableReason::Crash,
                    "sidecar exited mid-request",
                )),
                Err(_elapsed) => {
                    shared.on_timeout(ticket.id, ticket.epoch);
                    Err(Unavailable::new(
                        UnavailableReason::Timeout,
                        format!("no answer within {:?}", ticket.budget),
                    ))
                }
            }
        }
        .await;
        shared.record(result)
    }

    /// Blocking twin of [`SidecarClient::check`] with identical semantics. Safe from
    /// a plain thread or a per-connection `LocalSet`: it never touches a tokio
    /// runtime.
    pub fn check_blocking(
        &self,
        kind: SubjectKind,
        text: &str,
        name: Option<&str>,
    ) -> Result<Vec<WireFinding>, Unavailable> {
        let shared = self.shared();
        let (tx, rx) = mpsc::sync_channel(1);
        let result = (|| {
            let ticket = shared.submit(
                (Op::Check, Some(kind), Some(text), name),
                PendKind::Check,
                Responder::Sync(tx),
            )?;
            match rx.recv_timeout(ticket.budget) {
                Ok(delivery) => interpret_check(delivery),
                Err(mpsc::RecvTimeoutError::Disconnected) => Err(Unavailable::new(
                    UnavailableReason::Crash,
                    "sidecar exited mid-request",
                )),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    shared.on_timeout(ticket.id, ticket.epoch);
                    Err(Unavailable::new(
                        UnavailableReason::Timeout,
                        format!("no answer within {:?}", ticket.budget),
                    ))
                }
            }
        })();
        shared.record(result)
    }

    async fn round_trip(&self, op: Op) -> Result<(Response, Duration), Unavailable> {
        let shared = self.shared();
        let (tx, rx) = oneshot::channel();
        let started = Instant::now();
        let result = async {
            let ticket = shared.submit(
                (op, None, None, None),
                PendKind::Other,
                Responder::Async(tx),
            )?;
            match tokio::time::timeout(ticket.budget, rx).await {
                Ok(Ok(delivery)) => interpret_ok(delivery).map(|r| (r, started.elapsed())),
                Ok(Err(_closed)) => Err(Unavailable::new(
                    UnavailableReason::Crash,
                    "sidecar exited mid-request",
                )),
                Err(_elapsed) => {
                    shared.on_timeout(ticket.id, ticket.epoch);
                    Err(Unavailable::new(
                        UnavailableReason::Timeout,
                        format!("no answer within {:?}", ticket.budget),
                    ))
                }
            }
        }
        .await;
        shared.record(result)
    }

    /// Liveness probe; returns the round-trip time.
    pub async fn ping(&self) -> Result<Duration, Unavailable> {
        self.round_trip(Op::Ping).await.map(|(_, elapsed)| elapsed)
    }

    /// Versions reported by the sidecar.
    pub async fn version(&self) -> Result<SidecarVersion, Unavailable> {
        let (resp, _) = self.round_trip(Op::Version).await?;
        let result = resp.version.ok_or_else(|| {
            Unavailable::new(
                UnavailableReason::ProtocolError,
                "version reply carried no version",
            )
        });
        self.shared().record(result)
    }

    /// Supervision snapshot.
    pub fn health(&self) -> SidecarHealth {
        let shared = self.shared();
        let st = lock(&shared.state);
        let state = match (&st.phase, st.slot.as_ref()) {
            (Phase::Missing, _) => SidecarState::Missing,
            (Phase::Backoff { .. }, _) => SidecarState::Backoff,
            (Phase::Running, Some(slot)) if slot.confirmed => SidecarState::Healthy,
            (Phase::Running, _) => SidecarState::Starting,
        };
        SidecarHealth {
            state,
            respawns: st.respawns,
            consecutive_timeouts: st.consecutive_timeouts,
            late_replies: shared.late_replies.load(Ordering::Relaxed),
            last_unavailable: st.last_unavailable,
            binary: shared.binary.clone(),
        }
    }

    /// Pid of the live child, for tests that kill it from outside.
    #[doc(hidden)]
    pub fn child_pid_for_tests(&self) -> Option<u32> {
        lock(&self.shared().state)
            .slot
            .as_ref()
            .map(|slot| slot.child.id())
    }
}

/// Next backoff after `prev`: doubled, never below `min` or above `max`.
pub fn next_backoff(prev: Duration, min: Duration, max: Duration) -> Duration {
    prev.saturating_mul(2).clamp(min, max)
}

/// Backoff for the next crash given how long the dead child had been up: a child
/// healthy for `healthy_reset` starts over at `min`, otherwise it keeps doubling.
pub fn backoff_after_healthy(
    elapsed: Duration,
    healthy_reset: Duration,
    prev: Duration,
    min: Duration,
    max: Duration,
) -> Duration {
    if elapsed >= healthy_reset {
        min
    } else {
        next_backoff(prev, min, max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_reason_words_round_trip() {
        let all = UnavailableReason::ALL;
        assert_eq!(all.len(), 7);
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b, "ALL must list each reason once");
            }
        }
        for r in all {
            assert_eq!(
                UnavailableReason::from_word(&r.to_string()),
                Some(r),
                "{r} must round-trip through its word"
            );
        }
        assert_eq!(UnavailableReason::from_word("nope"), None);
        assert_eq!(UnavailableReason::from_word(""), None);
        assert_eq!(
            UnavailableReason::KilledForTimeouts.to_string(),
            "killed_for_timeouts"
        );
        let outages: Vec<UnavailableReason> = all.into_iter().filter(|r| r.is_outage()).collect();
        assert_eq!(
            outages,
            vec![UnavailableReason::Missing, UnavailableReason::Crash],
            "only Missing and Crash are outages"
        );
    }

    #[test]
    fn request_budget_scales_and_caps() {
        let home = tempfile::tempdir().unwrap();
        let opts = SidecarOptions::for_home(home.path());
        assert_eq!(opts.budget_per_kib, Duration::from_millis(10));
        assert_eq!(opts.budget_ceiling, Duration::from_secs(3));

        let ms = Duration::from_millis;
        assert_eq!(request_budget(&opts, true, 100, 0), ms(250));
        assert_eq!(request_budget(&opts, true, 50 * 1024, 0), ms(750));
        assert_eq!(request_budget(&opts, true, 10 * 1024, 40 * 1024), ms(750));
        assert_eq!(
            request_budget(&opts, true, 1024 * 1024, 0),
            Duration::from_secs(3),
            "capped at the ceiling"
        );
        assert_eq!(request_budget(&opts, false, 0, 0), opts.first_timeout);
        assert_eq!(
            request_budget(&opts, false, 1024 * 1024, 0),
            opts.first_timeout,
            "cold is first_timeout whatever the size"
        );
    }
}
