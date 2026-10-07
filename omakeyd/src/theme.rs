//! The desktop's Omarchy theme, for phones that colour themselves to match
//! (PROTOCOL.md, ACK `theme`). Read from Omarchy's state directory and
//! looked at again every few seconds, so a theme change reaches a phone
//! that is typing.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

/// The colors.toml keys sent, in wire order.
pub const COLOR_KEYS: [&str; 14] = [
    "background",
    "lighter_background",
    "dark_background",
    "foreground",
    "muted",
    "accent",
    "selection",
    "red",
    "yellow",
    "green",
    "cyan",
    "blue",
    "magenta",
    "orange",
];

const MAX_NAME: usize = 32;
const CHECK_EVERY: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub name: String,
    pub light: bool,
    pub colors: Vec<[u8; 3]>,
}

impl Theme {
    /// From theme.name and colors.toml. A missing colour borrows a close one
    /// (muted from foreground, lighter_background from background, …).
    pub fn parse(name: &str, toml: &str) -> Option<Theme> {
        let mut light = false;
        let mut found: Vec<(String, [u8; 3])> = Vec::new();
        for line in toml.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim().trim_matches('"'));
            if k == "mode" {
                light = v == "light";
            } else if let Some(rgb) = parse_hex(v) {
                found.push((k.to_string(), rgb));
            }
        }
        let get = |k: &str| found.iter().find(|(n, _)| n == k).map(|(_, c)| *c);
        let bg = get("background")?;
        let fg = get("foreground")?;
        let colors = COLOR_KEYS
            .iter()
            .map(|k| {
                get(k).unwrap_or(match *k {
                    "lighter_background" | "dark_background" | "selection" => bg,
                    "muted" => get("dark_foreground").unwrap_or(fg),
                    "accent" => get("blue").unwrap_or(fg),
                    _ => fg,
                })
            })
            .collect();
        let mut name = name.trim().to_string();
        while name.len() > MAX_NAME {
            name.pop();
        }
        Some(Theme { name, light, colors })
    }

    /// The ACK trailer: theme_len, then mode, name and colours.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = vec![self.light as u8, self.name.len() as u8];
        body.extend_from_slice(self.name.as_bytes());
        body.push(self.colors.len() as u8);
        for c in &self.colors {
            body.extend_from_slice(c);
        }
        let mut out = vec![body.len() as u8];
        out.extend(body);
        out
    }

    /// Reads [encode]'s output.
    pub fn decode(buf: &[u8]) -> Option<Theme> {
        let len = *buf.first()? as usize;
        let b = buf.get(1..1 + len)?;
        let light = *b.first()? == 1;
        let n = *b.get(1)? as usize;
        let name = String::from_utf8(b.get(2..2 + n)?.to_vec()).ok()?;
        let count = *b.get(2 + n)? as usize;
        let colors = b.get(3 + n..3 + n + count * 3)?.chunks(3).map(|c| [c[0], c[1], c[2]]).collect();
        Some(Theme { name, light, colors })
    }
}

fn parse_hex(v: &str) -> Option<[u8; 3]> {
    let h = v.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(h, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

/// Omarchy's current theme, read again when its files change.
pub struct ThemeWatch {
    dir: Option<PathBuf>,
    stamp: Option<(Option<SystemTime>, Option<SystemTime>)>,
    last_check: Option<Instant>,
}

impl ThemeWatch {
    pub fn new() -> ThemeWatch {
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")));
        ThemeWatch { dir: state.map(|s| s.join("omarchy/current")), stamp: None, last_check: None }
    }

    /// Some(new encoded trailer, or None without a theme) when it changed
    /// since the last call; None when it didn't or it isn't time to look.
    pub fn poll(&mut self, now: Instant) -> Option<Option<Vec<u8>>> {
        if self.last_check.is_some_and(|t| now.duration_since(t) < CHECK_EVERY) {
            return None;
        }
        self.last_check = Some(now);
        let dir = self.dir.as_ref()?;
        let name_path = dir.join("theme.name");
        let colors_path = dir.join("theme/colors.toml");
        let mtime = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let stamp = (mtime(&name_path), mtime(&colors_path));
        if self.stamp == Some(stamp) {
            return None;
        }
        self.stamp = Some(stamp);
        let name = std::fs::read_to_string(&name_path).unwrap_or_default();
        let theme = std::fs::read_to_string(&colors_path).ok().and_then(|t| Theme::parse(&name, &t));
        if let Some(t) = &theme {
            eprintln!("omakeyd: desktop theme {}", t.name);
        }
        Some(theme.map(|t| t.encode()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_omarchy_colors_and_round_trips() {
        let toml = "mode = \"light\"\n\naccent = \"#1e66f5\"\nbackground = \"#eff1f5\"\nforeground = \"#4c4f69\"\nred = \"#d20f39\"\n";
        let t = Theme::parse("catppuccin-latte\n", toml).unwrap();
        assert!(t.light);
        assert_eq!(t.name, "catppuccin-latte");
        assert_eq!(t.colors.len(), COLOR_KEYS.len());
        assert_eq!(t.colors[0], [0xef, 0xf1, 0xf5]);
        assert_eq!(t.colors[5], [0x1e, 0x66, 0xf5]);
        // Missing ones borrow: lighter_background from background, muted from foreground.
        assert_eq!(t.colors[1], t.colors[0]);
        assert_eq!(t.colors[4], t.colors[3]);
        let enc = t.encode();
        assert_eq!(enc[0] as usize, enc.len() - 1);
        assert_eq!(Theme::decode(&enc), Some(t));
    }

    #[test]
    fn needs_background_and_foreground() {
        assert!(Theme::parse("x", "accent = \"#123456\"").is_none());
    }
}
