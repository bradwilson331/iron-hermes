//! Pill color rotation per D-04: cyan, magenta, green, yellow, dimmed.
//! The hint (if provided) is ALWAYS dimmed regardless of rotation index.

use colored::{ColoredString, Colorize};

/// Rotate pill colors per D-04. Returns a `Vec<ColoredString>` — one entry per
/// input pill, plus one extra dimmed entry if `hint` is `Some`.
pub fn rotate_pill_colors(pills: &[String], hint: Option<&str>) -> Vec<ColoredString> {
    let palette: [fn(&str) -> ColoredString; 5] = [
        |s| s.cyan(),
        |s| s.magenta(),
        |s| s.green(),
        |s| s.yellow(),
        |s| s.dimmed(),
    ];
    let mut out: Vec<ColoredString> = pills
        .iter()
        .enumerate()
        .map(|(i, p)| palette[i % palette.len()](p.as_str()))
        .collect();
    if let Some(h) = hint {
        out.push(h.dimmed()); // hint always dimmed (D-04)
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pills(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("p{}", i)).collect()
    }

    #[test]
    fn empty_input_empty_output() {
        let out = rotate_pill_colors(&[], None);
        assert!(out.is_empty());
    }

    #[test]
    fn five_pills_five_outputs() {
        let out = rotate_pill_colors(&pills(5), None);
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn six_pills_wraps_to_cyan_at_index_5() {
        // palette has 5 entries; pill[5] must wrap to palette[0]=cyan.
        let out = rotate_pill_colors(&pills(6), None);
        assert_eq!(out.len(), 6);
        let expected_cyan = "p5".to_string().cyan().to_string();
        assert_eq!(out[5].to_string(), expected_cyan);
        let expected_cyan_0 = "p0".to_string().cyan().to_string();
        assert_eq!(out[0].to_string(), expected_cyan_0);
    }

    #[test]
    fn hint_is_appended_and_dimmed() {
        let out = rotate_pill_colors(&pills(3), Some("ctrl+p commands"));
        assert_eq!(out.len(), 4);
        let expected = "ctrl+p commands".dimmed().to_string();
        assert_eq!(out[3].to_string(), expected);
    }

    #[test]
    fn palette_order_is_cyan_magenta_green_yellow_dimmed() {
        let out = rotate_pill_colors(&pills(5), None);
        assert_eq!(out[0].to_string(), "p0".cyan().to_string());
        assert_eq!(out[1].to_string(), "p1".magenta().to_string());
        assert_eq!(out[2].to_string(), "p2".green().to_string());
        assert_eq!(out[3].to_string(), "p3".yellow().to_string());
        assert_eq!(out[4].to_string(), "p4".dimmed().to_string());
    }
}
