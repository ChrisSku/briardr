// The pairing link and its QR code: a new device opens Briardr with the one-time code already filled
// in. The code is in the link's fragment, which a browser never sends to a server or a proxy, so it
// reaches no access log.
use qrcode::{Color, EcLevel, QrCode};

/// The link a new device opens to pair: Briardr's address with the code after `#pair=`.
pub fn pairing_link(origin: &str, code: &str) -> String {
    format!("{}/#pair={code}", origin.trim_end_matches('/'))
}

/// The QR code's modules for `text`: one string per row, `1` dark and `0` light, without the quiet zone
/// around them. None when the text doesn't fit a QR code.
pub fn modules(text: &str) -> Option<Vec<String>> {
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M).ok()?;
    let width = code.width();
    let rows = code
        .to_colors()
        .chunks(width)
        .map(|row| {
            row.iter()
                .map(|c| if *c == Color::Dark { '1' } else { '0' })
                .collect()
        })
        .collect();
    Some(rows)
}

/// The QR code for a terminal: two rows of modules per line as half blocks, black on white whatever the
/// terminal's colours are (a camera reads dark on light), with the quiet zone.
pub fn for_terminal(text: &str) -> Option<String> {
    const QUIET: usize = 2;
    let rows = modules(text)?;
    let width = rows.first()?.len() + 2 * QUIET;
    let blank = "0".repeat(width);
    let mut padded: Vec<String> = vec![blank.clone(); QUIET];
    padded.extend(
        rows.iter()
            .map(|r| format!("{}{r}{}", "0".repeat(QUIET), "0".repeat(QUIET))),
    );
    padded.extend(vec![blank.clone(); QUIET]);
    if padded.len() % 2 == 1 {
        padded.push(blank);
    }
    let mut out = String::new();
    for pair in padded.chunks(2) {
        out.push_str("  \x1b[30;47m");
        for (top, bottom) in pair[0].bytes().zip(pair[1].bytes()) {
            out.push(match (top, bottom) {
                (b'1', b'1') => '█',
                (b'1', _) => '▀',
                (_, b'1') => '▄',
                _ => ' ',
            });
        }
        out.push_str("\x1b[0m\n");
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_link_carries_the_code_in_its_fragment() {
        assert_eq!(
            pairing_link("https://mac.tail1.ts.net:7443", "ABCD-EFGH"),
            "https://mac.tail1.ts.net:7443/#pair=ABCD-EFGH"
        );
        assert_eq!(
            pairing_link("https://briardr.example.com/", "ABCD-EFGH"),
            "https://briardr.example.com/#pair=ABCD-EFGH"
        );
    }

    #[test]
    fn modules_are_a_square_with_the_finder_patterns() {
        let rows = modules("https://mac.tail1.ts.net:7443/#pair=ABCD-EFGH").expect("a code");
        assert!(rows.len() >= 21 && rows.iter().all(|r| r.len() == rows.len()));
        // A finder pattern in three corners: a dark 7-module edge, then a light separator.
        assert!(rows[0].starts_with("11111110") && rows[0].ends_with("01111111"));
        assert!(rows[rows.len() - 1].starts_with("11111110"));
        assert_eq!(&rows[1][..8], "10000010");
        assert!(modules(&"x".repeat(5000)).is_none());
    }

    #[test]
    fn the_terminal_code_is_black_on_white_with_a_quiet_zone() {
        let text = for_terminal("https://localhost:7443/#pair=ABCD-EFGH").expect("a code");
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines
            .iter()
            .all(|l| l.starts_with("  \x1b[30;47m") && l.ends_with("\x1b[0m")));
        let cells = |l: &str| l.chars().filter(|c| "█▀▄ ".contains(*c)).count();
        let modules = modules("https://localhost:7443/#pair=ABCD-EFGH")
            .unwrap()
            .len();
        // Two leading spaces of indentation, then the modules and the quiet zone on both sides.
        assert!(lines.iter().all(|l| cells(l) == 2 + modules + 4));
        assert_eq!(lines.len(), (modules + 4).div_ceil(2));
        // The first line is only quiet zone.
        assert!(!lines[0].contains(['█', '▀', '▄']));
    }
}
