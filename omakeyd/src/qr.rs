//! QR codes for the pairing link: rows for the bar widget, blocks for the terminal.

use qrcode::{Color, EcLevel, QrCode};

/// One string per row, '1' = dark module. No quiet zone.
pub fn rows(data: &str) -> Vec<String> {
    let Ok(code) = QrCode::with_error_correction_level(data, EcLevel::M) else {
        return Vec::new();
    };
    let w = code.width();
    code.to_colors()
        .chunks(w)
        .map(|row| row.iter().map(|c| if *c == Color::Dark { '1' } else { '0' }).collect())
        .collect()
}

/// Render with half blocks, two rows per line, with a quiet zone. Light
/// modules are drawn so it scans on dark terminal themes too.
pub fn terminal(data: &str) -> String {
    let rows = rows(data);
    let n = rows.len();
    let q = 2;
    let dark = |y: isize, x: isize| -> bool {
        if y < 0 || x < 0 || y as usize >= n || x as usize >= n {
            return false;
        }
        rows[y as usize].as_bytes()[x as usize] == b'1'
    };
    let mut out = String::new();
    let mut y = -(q as isize);
    while y < (n + q) as isize {
        for x in -(q as isize)..(n + q) as isize {
            out.push(match (dark(y, x), dark(y + 1, x)) {
                (false, false) => '█',
                (true, false) => '▄',
                (false, true) => '▀',
                (true, true) => ' ',
            });
        }
        out.push('\n');
        y += 2;
    }
    out
}
