//! Per-session FIFO queue (Phase 36.17.1).
//!
//! Production data structure backing the `/queue` command and busy-agent
//! routing. Python parity: `gateway/run.py` §2304-2415 (`_enqueue_fifo`,
//! `_promote_queued_event`, `_queue_depth`) and §1007 (`_dequeue_pending_event`).
//!
//! See `.planning/phases/36.17.1-in-mem-fifo-queuing-parity-of-python-deque-for-chat-sessions/`
//! for the full spec. Symbols arrive in Plan 01 Task 2.
