//! Double-ctrl-c state machine per D-10..D-14.
//!
//! PURE function state machine — no tokio, no real SIGINT needed for tests (D-21).
//! The 1.5s window is a compile-time constant (D-12 / D-14 §Configuration).

use std::time::{Duration, Instant};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum CtrlCDecision {
    /// First ctrl-c while in-flight: cancel in-flight work, return to prompt. (D-11)
    CancelTurn,
    /// Second ctrl-c within window while in-flight: persist + exit 0. (D-12)
    ExitCleanly,
    /// Ctrl-c at prompt (not in-flight): print "^C — type /quit to exit" and loop. (D-14)
    ShowPromptHint,
}

pub struct DoubleCtrlCState {
    window: Duration,
    last_cancel_at: Option<Instant>,
}

impl Default for DoubleCtrlCState {
    fn default() -> Self {
        Self::new()
    }
}

impl DoubleCtrlCState {
    pub fn new() -> Self {
        Self {
            window: Duration::from_millis(1500),
            last_cancel_at: None,
        }
    }

    /// Returns the decision for THIS ctrl-c event.
    /// Caller tracks `in_flight` externally (derived from whether the agent future is running).
    pub fn on_ctrl_c(&mut self, now: Instant, in_flight: bool) -> CtrlCDecision {
        let within_window = self
            .last_cancel_at
            .map(|t| now.duration_since(t) < self.window)
            .unwrap_or(false);
        if !in_flight {
            if within_window {
                return CtrlCDecision::ExitCleanly;
            }
            return CtrlCDecision::ShowPromptHint;
        }
        if within_window {
            CtrlCDecision::ExitCleanly
        } else {
            self.last_cancel_at = Some(now);
            CtrlCDecision::CancelTurn
        }
    }

    /// Reset on successful turn completion OR on fresh user input (D-13).
    pub fn reset(&mut self) {
        self.last_cancel_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrlc_at_prompt_is_hint() {
        let mut s = DoubleCtrlCState::new();
        assert_eq!(
            s.on_ctrl_c(Instant::now(), false),
            CtrlCDecision::ShowPromptHint
        );
    }

    #[test]
    fn first_ctrlc_in_flight_cancels() {
        let mut s = DoubleCtrlCState::new();
        assert_eq!(s.on_ctrl_c(Instant::now(), true), CtrlCDecision::CancelTurn);
    }

    #[test]
    fn second_ctrlc_within_window_exits() {
        let mut s = DoubleCtrlCState::new();
        let t0 = Instant::now();
        s.on_ctrl_c(t0, true);
        let t1 = t0 + Duration::from_millis(500);
        assert_eq!(s.on_ctrl_c(t1, true), CtrlCDecision::ExitCleanly);
    }

    #[test]
    fn second_ctrlc_after_window_cancels_again() {
        let mut s = DoubleCtrlCState::new();
        let t0 = Instant::now();
        s.on_ctrl_c(t0, true);
        let t1 = t0 + Duration::from_millis(1600);
        assert_eq!(s.on_ctrl_c(t1, true), CtrlCDecision::CancelTurn);
    }

    #[test]
    fn reset_clears_window() {
        let mut s = DoubleCtrlCState::new();
        let t0 = Instant::now();
        s.on_ctrl_c(t0, true); // CancelTurn
        s.reset();
        let t1 = t0 + Duration::from_millis(100);
        assert_eq!(s.on_ctrl_c(t1, true), CtrlCDecision::CancelTurn);
    }

    #[test]
    fn second_ctrlc_at_prompt_within_window_exits() {
        let mut s = DoubleCtrlCState::new();
        let t0 = Instant::now();
        assert_eq!(s.on_ctrl_c(t0, true), CtrlCDecision::CancelTurn);
        let t1 = t0 + Duration::from_millis(500);
        assert_eq!(s.on_ctrl_c(t1, false), CtrlCDecision::ExitCleanly);
    }

    #[test]
    fn three_rapid_presses_within_window() {
        let mut s = DoubleCtrlCState::new();
        let t0 = Instant::now();
        assert_eq!(s.on_ctrl_c(t0, true), CtrlCDecision::CancelTurn);
        let t1 = t0 + Duration::from_millis(200);
        assert_eq!(s.on_ctrl_c(t1, true), CtrlCDecision::ExitCleanly);
        let t2 = t0 + Duration::from_millis(400);
        // Third press still within window; caller has not reset — state still
        // returns ExitCleanly. The main.rs emergency-exit escape hatch (Plan 21-03)
        // handles the real 3rd-press footgun via a separate 3s window + exit(130).
        assert_eq!(s.on_ctrl_c(t2, true), CtrlCDecision::ExitCleanly);
    }
}
