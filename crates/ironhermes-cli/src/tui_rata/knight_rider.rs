//! Knight Rider scanner frame generator (pure function, no I/O).
//!
//! Per D-07: 10-cell horizontal track, triangle-wave sweep, lit cell bright cyan,
//! trailing cells fade via dimmed. Frame rate is driven externally (21-02's render
//! task ticks every 100ms per D-07).
//!
//! Verbatim lift from `tui_rata/knight_rider.rs` — per D-11 (Phase 22.4 plan 22.4-01).

use colored::Colorize;

pub const TRACK_WIDTH: usize = 10;

/// Given a monotonic tick, produce the 10-cell Knight Rider frame.
/// Triangle wave: lit cell sweeps 0 → 9 → 0 → 9 over (TRACK_WIDTH-1)*2 = 18 ticks.
pub fn frame(tick: u64) -> String {
    let period = (TRACK_WIDTH as u64 - 1) * 2;
    let phase = tick % period;
    let lit = if phase < TRACK_WIDTH as u64 {
        phase as usize
    } else {
        (period - phase) as usize
    };

    (0..TRACK_WIDTH)
        .map(|i| {
            let distance = (i as i32 - lit as i32).unsigned_abs() as usize;
            match distance {
                0 => "█".bright_cyan().to_string(),
                1 => "▓".cyan().to_string(),
                2 => "▒".cyan().dimmed().to_string(),
                _ => "░".dimmed().to_string(),
            }
        })
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Count how many glyphs from the knight-rider set appear, ignoring ANSI escapes.
    fn glyph_count(s: &str) -> usize {
        s.chars().filter(|c| ['█', '▓', '▒', '░'].contains(c)).count()
    }

    #[test]
    fn frame_has_track_width_glyphs() {
        for tick in 0..30 {
            assert_eq!(
                glyph_count(&frame(tick)),
                TRACK_WIDTH,
                "tick {} must produce {} glyphs",
                tick,
                TRACK_WIDTH
            );
        }
    }

    #[test]
    fn triangle_wave_reaches_both_endpoints() {
        let mut positions = std::collections::HashSet::new();
        for tick in 0..18u64 {
            let period = 18u64;
            let phase = tick % period;
            let lit = if phase < TRACK_WIDTH as u64 {
                phase as usize
            } else {
                (period - phase) as usize
            };
            positions.insert(lit);
        }
        assert!(positions.contains(&0), "lit never reaches 0");
        assert!(positions.contains(&(TRACK_WIDTH - 1)), "lit never reaches {}", TRACK_WIDTH - 1);
    }

    #[test]
    fn period_is_stable_frames_18_and_0_match() {
        assert_eq!(frame(0), frame(18));
    }
}
