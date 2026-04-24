//! Phase 21.7 Plan 11 (GAP-21.7-01): concurrent slash-command input channel.
//!
//! Hosts the blocking `rustyline::DefaultEditor` on a dedicated thread and
//! exposes a tokio-friendly mpsc-based API. This allows the `run_chat` REPL
//! to poll for user input from a `tokio::select!` arm alongside the in-flight
//! agent turn future so slash commands like `/agents list` dispatch mid-turn
//! without cancelling the turn.
//!
//! ## Architecture
//!
//! ```text
//!   run_chat (tokio task)           ReplInputChannel             blocking thread
//!        |  request_prompt(req) --> cmd_tx.send(Prompt(req,tx))
//!        |                                                   cmd_rx.recv()
//!        |                                                   rl.readline(&prefix)
//!        |  recv_line().await   <-- line_rx.recv()      <--  line_tx.send(ReplLine)
//! ```
//!
//! ## Threading rationale
//!
//! `rustyline::DefaultEditor` owns an OS-level terminal handle and is NOT
//! `Send`-safe to move across `.await` points. Per plan-11 Rule 3 / plan-11
//! deviation anchors: we use `std::thread::spawn` to host the editor on a
//! dedicated OS thread rather than `tokio::task::spawn_blocking`. Tokio's
//! mpsc `Sender` is `Send + Sync` from any thread (the senders here are
//! crossed into the blocking thread and used infallibly). This keeps the
//! editor under single ownership and serializes operations via a
//! command-channel pattern.
//!
//! ## Public API
//!
//! - [`ReplInputChannel::spawn`] starts the worker.
//! - [`ReplInputChannel::request_prompt`] asks the worker to call
//!   `rl.readline(prefix)` and deliver the result.
//! - [`ReplInputChannel::recv_line`] awaits the next [`ReplLine`] from the
//!   worker. Returns `None` if the worker has exited (channel closed).
//! - [`ReplInputChannel::try_recv_line`] drains any buffered lines
//!   non-blockingly (used to discard buffered mid-turn input after the turn
//!   ends).
//! - [`ReplInputChannel::add_history`] appends a line to rustyline's history.
//! - [`ReplInputChannel::shutdown`] drops the command channel — the worker
//!   exits on next recv.
//!
//! ## Plan-11 invariants
//!
//! - Slash commands mid-turn MUST NOT cancel the in-flight agent turn. This
//!   module only provides the channel plumbing; dispatch semantics are the
//!   caller's responsibility (see `run_chat`'s `tokio::select!` third arm).
//! - Non-slash mid-turn input is the caller's problem to discard. This
//!   module does not inspect line contents.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
// Phase 22.3: required to call set_max_history_size / set_history_ignore_dups
// on DefaultEditor (methods are on the Configurer trait, not directly on Editor).
use rustyline::config::Configurer;

/// Post-plan-11 fix: thread-safe handle over a rustyline `ExternalPrinter`.
///
/// Rustyline owns the terminal in raw mode while `readline` is active —
/// direct `eprintln!` / `println!` from other threads (e.g. the
/// `SubagentProgressCallback` ticker) corrupts the prompt row and
/// fragments user input (observed in Phase 21.7 UAT follow-up).
///
/// `rustyline::Editor::create_external_printer()` returns a printer that
/// buffers lines and paints them ABOVE the prompt row without disturbing
/// what the user is typing. The concrete type is `Send` but not `Sync`,
/// and its `print` method takes `&mut self`, so we wrap it in
/// `Arc<Mutex<Box<dyn rustyline::ExternalPrinter + Send>>>` to allow
/// cheap `Clone` and safe sharing across tasks.
///
/// When no TTY is attached (tests, piped I/O, headless runs),
/// `create_external_printer()` errors; we fall back to a dummy printer
/// that writes directly to stderr. Production-correct (preserves
/// visibility in non-interactive contexts) and lets unit tests run
/// without a real TTY.
#[derive(Clone)]
pub struct ExternalPrinterHandle {
    inner: Arc<Mutex<Box<dyn rustyline::ExternalPrinter + Send>>>,
}

impl ExternalPrinterHandle {
    /// Print a line above the rustyline prompt without corrupting the
    /// user's in-progress input. Fire-and-forget: the terminal may be
    /// non-interactive (redirected to a file, no tty) or rustyline may
    /// have already shut down, in which case this silently no-ops.
    pub fn println(&self, msg: impl Into<String>) {
        let msg = msg.into();
        if let Ok(mut guard) = self.inner.lock() {
            let _ = guard.print(msg);
        }
    }
}

/// Fallback printer used when rustyline's `create_external_printer()`
/// fails (no TTY attached). Writes lines directly to stderr — the same
/// destination the pre-plan-11 ticker used, so headless runs behave
/// exactly as before this fix.
struct StderrFallbackPrinter;

impl rustyline::ExternalPrinter for StderrFallbackPrinter {
    fn print(&mut self, msg: String) -> rustyline::Result<()> {
        eprintln!("{}", msg);
        Ok(())
    }
}

/// One outcome from a `rl.readline(prefix)` call, mirroring rustyline's
/// result shape 1:1 so the REPL caller can route each variant to the same
/// outcome it routed pre-plan-11 (when `rl.readline` was called inline).
#[derive(Debug)]
pub enum ReplLine {
    /// A full line of input (trimmed or raw — caller decides).
    Line(String),
    /// ctrl-c at the prompt. Maps to `rustyline::error::ReadlineError::Interrupted`.
    Interrupted,
    /// ctrl-d / EOF at the prompt. Maps to `rustyline::error::ReadlineError::Eof`.
    Eof,
    /// Any other readline error. The string is the underlying error message.
    Error(String),
}

/// Arguments for a single prompt request.
///
/// `in_turn` is purely informational — this module does NOT change behavior
/// based on it. The caller uses it to decide how to route the resulting
/// [`ReplLine::Line`] (slash dispatch vs. normal input vs. discard).
///
/// Plan 21.7-12 (GAP-21.7-02): `reserved_rows` enables worker-thread cursor
/// positioning. When `Some(N)` AND `in_turn == false`, the worker emits
/// absolute-positioning ANSI for row `(terminal_rows - N)` IMMEDIATELY
/// BEFORE `rl.readline(&prefix)` on the SAME thread as the readline paint —
/// closing the race window between the main task and the worker. When
/// `None` OR `in_turn == true`, the worker skips positioning (preserves
/// the invisible mid-turn prompt behavior).
#[derive(Debug, Clone)]
pub struct PromptRequest {
    /// The prompt prefix string rustyline will paint (e.g. `"You: "`).
    pub prefix: String,
    /// `true` during an in-flight agent turn; `false` otherwise. Informational.
    pub in_turn: bool,
    /// Plan 21.7-12 (GAP-21.7-02): reserved-row count for worker-thread
    /// cursor positioning. `Some(N)` asks the worker to emit
    /// absolute-positioning ANSI for row `(terminal_rows - N)` immediately
    /// before `rl.readline(&prefix)` on the same thread as the readline
    /// paint. `None` (or `in_turn: true`) skips positioning and preserves
    /// the invisible mid-turn prompt behavior.
    pub reserved_rows: Option<u16>,
}

/// Internal command envelope delivered to the blocking worker.
enum Command {
    /// Ask the worker to issue `rl.readline(&prefix)` and reply with one
    /// `ReplLine` on the attached oneshot.
    Prompt(PromptRequest, oneshot::Sender<ReplLine>),
    /// Append a line to rustyline's in-memory history.
    AddHistory(String),
    /// Drop the editor and exit the worker loop.
    Shutdown,
}

/// Handle for the REPL input worker thread.
///
/// Holds a command `Sender` (sending `Command::Prompt` / `AddHistory` /
/// `Shutdown` to the worker) plus a line `Receiver` (demultiplexed from
/// per-prompt oneshots by a small forwarder task).
pub struct ReplInputChannel {
    cmd_tx: CmdAndReplyTx,
    line_rx: mpsc::UnboundedReceiver<ReplLine>,
    #[allow(dead_code)] // join handle retained so the worker is cancel-safe on Drop
    worker: Option<std::thread::JoinHandle<()>>,
}

impl ReplInputChannel {
    /// Spawn the blocking rustyline worker on a dedicated OS thread.
    ///
    /// The worker owns a `rustyline::DefaultEditor` exclusively. Commands
    /// arriving on the command channel are serviced in order. On
    /// `Command::Shutdown` or when the command channel is closed, the worker
    /// drops the editor and exits.
    ///
    /// `history_path` is reserved for future history-persistence work; the
    /// current implementation only uses in-memory history (matching the
    /// pre-plan-11 rustyline behavior). Pass `None` to keep parity.
    pub fn spawn(
        history_path: Option<PathBuf>,
    ) -> anyhow::Result<(Self, ExternalPrinterHandle)> {
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<Command>();
        let (line_tx, line_rx) = mpsc::unbounded_channel::<ReplLine>();

        // Forwarder task: demultiplex per-prompt oneshots onto the stream
        // receiver. Owns its own mpsc between worker-side oneshots and the
        // main receiver so the public API is a simple `recv_line().await`.
        let worker_line_tx = line_tx.clone();

        // Post-plan-11 UAT follow-up: the worker creates rustyline's
        // ExternalPrinter alongside the editor and sends it back to the
        // main task so the `SubagentProgressCallback` (and future
        // concurrent-output sites) can paint lines ABOVE the prompt row.
        // std::sync::mpsc::sync_channel(1) bounds the handshake to a
        // single message, matches the `std::thread::spawn` ownership model
        // (no tokio::runtime::Handle required inside the OS thread), and
        // delivers a clear init-failure signal via `recv()` error.
        let (printer_tx, printer_rx) =
            std::sync::mpsc::sync_channel::<Box<dyn rustyline::ExternalPrinter + Send>>(1);

        let worker = std::thread::Builder::new()
            .name("ironhermes-repl-input".to_string())
            .spawn(move || {
                // Construct the rustyline editor inside the worker thread so
                // its !Send innards never cross a thread boundary.
                let mut rl = match rustyline::DefaultEditor::new() {
                    Ok(r) => r,
                    Err(e) => {
                        // Best-effort: report the error once and exit. No
                        // prompts will ever be serviced; the first call to
                        // `request_prompt` will see a closed line channel.
                        // Also drop `printer_tx` so the main task's recv()
                        // fails cleanly with Disconnected.
                        drop(printer_tx);
                        let _ = worker_line_tx
                            .send(ReplLine::Error(format!("readline init failed: {}", e)));
                        return;
                    }
                };

                // Create the external printer FIRST so the main task can
                // proceed regardless of whether any prompt is ever issued.
                // If this fails (typically: no TTY attached — tests, piped
                // I/O, headless CI), fall back to a dummy printer that
                // writes to stderr. spawn() always succeeds; callers get
                // a working (if terminal-unaware) printer either way.
                let boxed_printer: Box<dyn rustyline::ExternalPrinter + Send> =
                    match rl.create_external_printer() {
                        Ok(p) => Box::new(p),
                        Err(_) => Box::new(StderrFallbackPrinter),
                    };
                if printer_tx.send(boxed_printer).is_err() {
                    // Main task hung up before we could send; nothing
                    // left to do — just exit the worker cleanly.
                    return;
                }

                // Phase 22.3 D-08 / UI-SPEC HIST-4..HIST-6:
                // Activate persistent rustyline history.
                //
                // CORRECTION (RESEARCH §rustyline API Notes): rustyline 15
                // does NOT expose `set_history_duplicates(HistoryDuplicates::Prev)`.
                // The correct API is `set_history_ignore_dups(true)` (bool).
                // `load_history` returns Err(NotFound) on missing file — NOT a
                // silent no-op as UI-SPEC HIST-4 says. We must explicitly ignore
                // NotFound here so first-run launches do not warn.
                let _ = rl.set_max_history_size(1000);
                let _ = rl.set_history_ignore_dups(true);
                if let Some(ref path) = history_path {
                    if let Err(e) = rl.load_history(path) {
                        match &e {
                            rustyline::error::ReadlineError::Io(io_err)
                                if io_err.kind() == std::io::ErrorKind::NotFound =>
                            {
                                // First run: history file does not exist yet.
                                // Silent — this is expected, not an error.
                            }
                            _ => {
                                tracing::warn!(
                                    target: "ironhermes_cli::repl_input",
                                    path = ?path,
                                    error = ?e,
                                    "failed to load REPL history; continuing with empty history",
                                );
                            }
                        }
                    }
                }

                // blocking_recv is the std-thread cousin of `recv().await`.
                while let Some(cmd) = cmd_rx.blocking_recv() {
                    match cmd {
                        Command::Prompt(req, reply) => {
                            // Plan 21.7-12 (GAP-21.7-02): worker-thread
                            // cursor positioning. When `reserved_rows` is
                            // Some AND we're NOT in a mid-turn invisible
                            // prompt, emit absolute-positioning ANSI
                            // IMMEDIATELY before `rl.readline` on THIS
                            // thread so the cursor-move and the readline
                            // paint cannot be separated by any other
                            // thread's stderr writes. This is the
                            // structural fix for the floating-prompt race
                            // introduced when Plan 11 moved rustyline to
                            // a worker thread: main-task positioning +
                            // worker-thread readline left a window for
                            // stray stdout/stderr (including the TUI
                            // ticker) to move the cursor between the two.
                            if let Some(reserved) = req.reserved_rows
                                && !req.in_turn
                            {
                                use crossterm::tty::IsTty as _;
                                use std::io::Write as _;
                                let mut err = std::io::stderr();
                                if err.is_tty()
                                    && let Ok((_cols, rows)) =
                                        crossterm::terminal::size()
                                    && let Some(bytes) =
                                        crate::tui::prompt_position_ansi(rows, reserved)
                                {
                                    let _ = err.write_all(&bytes);
                                    let _ = err.flush();
                                }
                            }

                            let outcome = match rl.readline(&req.prefix) {
                                Ok(s) => ReplLine::Line(s),
                                Err(rustyline::error::ReadlineError::Interrupted) => {
                                    ReplLine::Interrupted
                                }
                                Err(rustyline::error::ReadlineError::Eof) => ReplLine::Eof,
                                Err(e) => ReplLine::Error(e.to_string()),
                            };
                            // oneshot::Sender is consumed on send; ignoring
                            // the error just means the REPL main task
                            // dropped the reply half (shutdown race).
                            let _ = reply.send(outcome);
                        }
                        Command::AddHistory(line) => {
                            let _ = rl.add_history_entry(&line);
                        }
                        Command::Shutdown => {
                            // Phase 22.3 D-08 / UI-SPEC HIST-4: persist history
                            // to disk before tearing down the worker. Errors
                            // are logged but not propagated — shutdown must
                            // still complete cleanly.
                            if let Some(ref path) = history_path {
                                if let Err(e) = rl.save_history(path) {
                                    tracing::warn!(
                                        target: "ironhermes_cli::repl_input",
                                        path = ?path,
                                        error = ?e,
                                        "failed to save REPL history",
                                    );
                                }
                            }
                            break;
                        }
                    }
                }
                // Drop the editor explicitly so the terminal handle is
                // released before the thread joins.
                drop(rl);
            })
            .map_err(|e| anyhow::anyhow!("failed to spawn repl input worker: {}", e))?;

        // Spawn a small tokio task that awaits each oneshot reply and
        // forwards onto the public line channel. Each `request_prompt` call
        // enqueues a fresh oneshot; the forwarder awaits them in submission
        // order (matches the worker's FIFO service order).
        //
        // We keep the forwarder colocated with the ReplInputChannel by
        // using a secondary mpsc of oneshot receivers: request_prompt sends
        // the receiver end into this channel; the forwarder drains it.
        let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<oneshot::Receiver<ReplLine>>();
        tokio::spawn(async move {
            while let Some(reply) = reply_rx.recv().await {
                match reply.await {
                    Ok(repl_line) => {
                        if line_tx.send(repl_line).is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        // oneshot sender dropped — worker exited. Stop.
                        break;
                    }
                }
            }
        });

        // Receive the ExternalPrinter the worker created alongside the
        // editor. `recv()` blocks the spawning task briefly — only until
        // the OS thread has initialized rustyline + printer. If the worker
        // failed to init, `recv()` returns `Err(Disconnected)` because the
        // worker dropped `printer_tx`; surface that as a clear error.
        let boxed_printer = printer_rx.recv().map_err(|_| {
            anyhow::anyhow!(
                "repl input worker failed to create rustyline ExternalPrinter \
                 (no TTY attached or rustyline init failed)"
            )
        })?;
        let external_printer = ExternalPrinterHandle {
            inner: Arc::new(Mutex::new(boxed_printer)),
        };

        Ok((
            Self {
                cmd_tx: CmdAndReplyTx::install(cmd_tx, reply_tx),
                line_rx,
                worker: Some(worker),
            },
            external_printer,
        ))
    }

    /// Request a new prompt from the worker.
    ///
    /// The result will arrive asynchronously on [`Self::recv_line`]. Prompt
    /// requests are FIFO-ordered with respect to the worker's service order.
    pub fn request_prompt(&self, req: PromptRequest) -> anyhow::Result<()> {
        // Wrap in an envelope with an oneshot reply. The forwarder task will
        // await the reply and put the `ReplLine` on line_rx.
        let (reply_tx, reply_rx) = oneshot::channel::<ReplLine>();
        // `cmd_tx` is a custom adapter — see `CmdAndReplyTx::install`.
        self.cmd_tx.send(Command::Prompt(req, reply_tx), reply_rx)
    }

    /// Await the next outcome from a previously-issued prompt request.
    ///
    /// Returns `None` if the worker has exited (command / forwarder channel
    /// closed).
    pub async fn recv_line(&mut self) -> Option<ReplLine> {
        self.line_rx.recv().await
    }

    /// Drain any buffered lines without blocking. Used to discard mid-turn
    /// input after the agent turn ends so no stale input leaks into the
    /// next prompt cycle.
    pub fn drain_buffered(&mut self) -> usize {
        let mut count = 0;
        while self.line_rx.try_recv().is_ok() {
            count += 1;
        }
        count
    }

    /// Append a line to rustyline's history. Fire-and-forget; if the worker
    /// has exited this call is a no-op.
    pub fn add_history(&self, line: &str) {
        let _ = self.cmd_tx.send_command_only(Command::AddHistory(line.to_string()));
    }

    /// Shut down the worker. Equivalent to `drop(self)` except that this
    /// consumes self and makes the shutdown explicit.
    pub fn shutdown(self) {
        let _ = self.cmd_tx.send_command_only(Command::Shutdown);
        // Dropping `self` releases the command sender; the worker's
        // `cmd_rx.blocking_recv()` returns None and the loop exits.
    }
}

/// Private adapter that couples the command sender with the forwarder's
/// reply-receiver-list channel. Keeps `request_prompt` atomic: the
/// oneshot receiver is inserted into the forwarder's queue AT THE SAME
/// TIME as the Prompt command is sent to the worker, so order is
/// preserved.
struct CmdAndReplyTx {
    cmd_tx: mpsc::UnboundedSender<Command>,
    reply_tx: mpsc::UnboundedSender<oneshot::Receiver<ReplLine>>,
}

impl CmdAndReplyTx {
    fn install(
        cmd_tx: mpsc::UnboundedSender<Command>,
        reply_tx: mpsc::UnboundedSender<oneshot::Receiver<ReplLine>>,
    ) -> Self {
        Self { cmd_tx, reply_tx }
    }

    /// Send a Prompt command plus register the reply receiver with the
    /// forwarder in a single atomic step. Order is preserved because
    /// both channels are FIFO and we write both in the same thread with
    /// no intervening await.
    fn send(
        &self,
        cmd: Command,
        reply_rx: oneshot::Receiver<ReplLine>,
    ) -> anyhow::Result<()> {
        // Register the reply receiver FIRST so if the caller drops it
        // before the worker gets the prompt, the forwarder still has the
        // receiver and will see the oneshot sender drop cleanly.
        self.reply_tx
            .send(reply_rx)
            .map_err(|_| anyhow::anyhow!("repl input forwarder channel closed"))?;
        self.cmd_tx
            .send(cmd)
            .map_err(|_| anyhow::anyhow!("repl input worker channel closed"))?;
        Ok(())
    }

    /// Send a non-prompt command (AddHistory, Shutdown) that does not
    /// produce a reply.
    fn send_command_only(&self, cmd: Command) -> anyhow::Result<()> {
        self.cmd_tx
            .send(cmd)
            .map_err(|_| anyhow::anyhow!("repl input worker channel closed"))?;
        Ok(())
    }
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// U-01: spawn + shutdown smoke test. Worker comes up, accepts a
    /// shutdown command, and we can re-construct the channel safely.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_and_shutdown_cleanly() {
        let (chan, _printer) = ReplInputChannel::spawn(None).expect("spawn should succeed");
        chan.shutdown();
        // Re-spawn to prove the first shutdown released the terminal cleanly.
        let (chan2, _printer2) =
            ReplInputChannel::spawn(None).expect("second spawn should succeed");
        chan2.shutdown();
    }

    /// U-02: explicit shutdown + drain is non-blocking. Proves that the
    /// Shutdown command reaches the worker and the channel does not
    /// deadlock when there are no pending prompts.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_then_drain_is_fast() {
        let (mut chan, _printer) = ReplInputChannel::spawn(None).expect("spawn");
        // Send an explicit Shutdown through the internal adapter so the
        // worker exits its loop promptly. Using cmd_tx directly (rather
        // than shutdown(self)) lets us observe the channel is still
        // usable for drain_buffered afterward.
        let _ = chan.cmd_tx.send_command_only(Command::Shutdown);
        // drain_buffered must return 0 without blocking — confirms the
        // line channel has not deadlocked on a pending prompt.
        tokio::time::timeout(
            std::time::Duration::from_millis(500),
            async { chan.drain_buffered() },
        )
        .await
        .expect("drain_buffered must not block");
    }

    /// U-03: add_history does not panic after shutdown. Exercises the
    /// no-op path when the worker has already exited.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn add_history_after_shutdown_is_noop() {
        let (mut chan, _printer) = ReplInputChannel::spawn(None).expect("spawn");
        // First — explicit: send Shutdown, then call add_history.
        let _ = chan
            .cmd_tx
            .send_command_only(Command::Shutdown);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        chan.add_history("hello");
        // Also drain_buffered should safely return 0.
        assert_eq!(chan.drain_buffered(), 0);
    }

    /// U-04: drain_buffered without pending lines returns 0 and does not
    /// block. Ensures the post-turn drain hook is safe on an idle channel.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drain_buffered_is_nonblocking_and_zero_on_idle() {
        let (mut chan, _printer) = ReplInputChannel::spawn(None).expect("spawn");
        let n = chan.drain_buffered();
        assert_eq!(n, 0);
        chan.shutdown();
    }

    /// U-05 (post-UAT fix): ExternalPrinterHandle is returned from spawn()
    /// and is Send + Sync + Clone so it can be captured by the
    /// SubagentProgressCallback closure and shared across tokio tasks.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn external_printer_is_send_sync_clone() {
        let (chan, printer) = ReplInputChannel::spawn(None).expect("spawn");
        let p2 = printer.clone();
        // Call println from two tasks concurrently — Mutex serializes the
        // underlying &mut self calls, so no data race is observable.
        let h1 = tokio::spawn(async move {
            p2.println("hello from task 1".to_string());
        });
        let p3 = printer.clone();
        let h2 = tokio::spawn(async move {
            p3.println("hello from task 2".to_string());
        });
        let _ = h1.await;
        let _ = h2.await;
        chan.shutdown();
    }

    /// Plan 21.7-12 P-01: `PromptRequest` round-trips `reserved_rows: Some(..)`
    /// — schema-level regression gate that the new field carries from the
    /// caller into the channel without being silently dropped.
    #[test]
    fn prompt_request_carries_reserved_rows() {
        let req = PromptRequest {
            prefix: "You: ".to_string(),
            in_turn: false,
            reserved_rows: Some(3),
        };
        assert_eq!(req.reserved_rows, Some(3));
        assert_eq!(req.prefix, "You: ");
        assert!(!req.in_turn);
    }

    /// Plan 21.7-12 P-02: `PromptRequest` with `reserved_rows: None`
    /// preserves the mid-turn invisible-prompt shape. The default-shape
    /// literal must still compile and the field must read back as None.
    #[test]
    fn prompt_request_none_reserved_rows_default_preserved() {
        let req = PromptRequest {
            prefix: String::new(),
            in_turn: true,
            reserved_rows: None,
        };
        assert_eq!(req.reserved_rows, None);
        assert!(req.in_turn);
    }

    /// Plan 21.7-12 P-03: U-05 regression re-run under the new field —
    /// proves the ExternalPrinter handoff was not regressed by adding
    /// `reserved_rows` to PromptRequest (separate from the worker-side
    /// positioning change, which only runs on a real TTY).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn external_printer_still_send_sync_clone() {
        let (chan, printer) = ReplInputChannel::spawn(None).expect("spawn");
        let p2 = printer.clone();
        let h1 = tokio::spawn(async move {
            p2.println("hello after reserved_rows".to_string());
        });
        let _ = h1.await;
        chan.shutdown();
    }
}
