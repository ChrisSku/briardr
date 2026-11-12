// The program's input modes, read from its raw output (herdr-core passes it on, and its first paint
// replays the modes). herdr-core never encodes keys, so Briardr does it for these modes, as a
// terminal would (`keys.rs`). Mouse and focus reporting are taken out of what the browser's terminal
// gets: Briardr reports the mouse and focus itself, and the browser's would report them again.
const ESC: u8 = 0x1b;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;
const BEL: u8 = 0x07;
/// Control sequences are short; a longer one is passed on unexamined.
const MAX_CSI: usize = 64;
/// How many kitty keyboard flag sets a screen remembers (as Ghostty and kitty do).
const KITTY_STACK: usize = 8;
/// DEC private modes for mouse and focus reporting, which the browser's terminal never gets.
const REPORTING: [u32; 10] = [9, 1000, 1001, 1002, 1003, 1004, 1005, 1006, 1015, 1016];

/// Which mouse events the program asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tracking {
    #[default]
    Off,
    /// Presses only (mode 9).
    X10,
    /// Presses and releases (1000).
    Normal,
    /// Also drags (1002).
    Button,
    /// Also movement (1003).
    Any,
}

/// How mouse events are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseEncoding {
    #[default]
    X10,
    /// 1005
    Utf8,
    /// 1006, and 1016 (pixels, sent here as cells)
    Sgr,
    /// 1015
    Urxvt,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Modes {
    /// DECCKM (1): cursor keys send SS3.
    pub app_cursor: bool,
    /// 2004
    pub bracketed_paste: bool,
    /// 1049, 1047 or 47
    pub alt_screen: bool,
    pub tracking: Tracking,
    pub encoding: MouseEncoding,
    /// Kitty keyboard flags pushed on the main and the alternate screen; each keeps its own.
    kitty: [Vec<u32>; 2],
}

impl Modes {
    /// The kitty keyboard protocol's flags for the screen in use (0: legacy keys).
    pub fn kitty_flags(&self) -> u32 {
        self.kitty[usize::from(self.alt_screen)]
            .last()
            .copied()
            .unwrap_or(0)
    }

    pub fn wants_mouse(&self) -> bool {
        self.tracking != Tracking::Off
    }

    fn set_dec(&mut self, mode: u32, on: bool) {
        match mode {
            1 => self.app_cursor = on,
            2004 => self.bracketed_paste = on,
            47 | 1047 | 1049 => self.alt_screen = on,
            9 | 1000 | 1002 | 1003 => {
                self.tracking = match (on, mode) {
                    (false, _) => Tracking::Off,
                    (true, 9) => Tracking::X10,
                    (true, 1000) => Tracking::Normal,
                    (true, 1002) => Tracking::Button,
                    _ => Tracking::Any,
                }
            }
            1005 | 1006 | 1015 | 1016 => {
                self.encoding = match (on, mode) {
                    (false, _) => MouseEncoding::X10,
                    (true, 1005) => MouseEncoding::Utf8,
                    (true, 1015) => MouseEncoding::Urxvt,
                    _ => MouseEncoding::Sgr,
                }
            }
            _ => {}
        }
    }

    /// `CSI > flags u` pushes, `CSI < n u` pops, `CSI = flags ; how u` sets, adds or removes.
    fn kitty(&mut self, prefix: u8, params: &[Option<u32>]) {
        let stack = &mut self.kitty[usize::from(self.alt_screen)];
        let arg = |i: usize| params.get(i).copied().flatten();
        match prefix {
            b'>' => {
                if stack.len() == KITTY_STACK {
                    stack.remove(0);
                }
                stack.push(arg(0).unwrap_or(0));
            }
            b'<' => {
                let n = arg(0).unwrap_or(1).max(1) as usize;
                stack.truncate(stack.len().saturating_sub(n));
            }
            b'=' => {
                let flags = arg(0).unwrap_or(0);
                let now = stack.last().copied().unwrap_or(0);
                let next = match arg(1).unwrap_or(1) {
                    2 => now | flags,
                    3 => now & !flags,
                    _ => flags,
                };
                match stack.last_mut() {
                    Some(top) => *top = next,
                    None => stack.push(next),
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    #[default]
    Ground,
    /// After ESC.
    Esc,
    /// Inside CSI, held back until its final byte.
    Csi,
    /// A CSI too long to examine, passed on until its final byte.
    CsiPass,
    /// Inside OSC, DCS, APC, PM or SOS, passed on as it comes.
    Str,
    /// ESC inside a string: ST, or the start of another sequence.
    StrEsc,
}

/// Reads one stream's output across chunks: a sequence may arrive split.
#[derive(Debug, Default)]
pub struct Vt {
    pub modes: Modes,
    state: State,
    /// The escape sequence being read.
    seq: Vec<u8>,
}

impl Vt {
    /// Reads program output and returns what the browser's terminal gets.
    pub fn feed(&mut self, input: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(input.len());
        for &b in input {
            self.step(b, &mut out);
        }
        out
    }

    fn step(&mut self, b: u8, out: &mut Vec<u8>) {
        match self.state {
            State::Ground => {
                if b == ESC {
                    self.seq.push(b);
                    self.state = State::Esc;
                } else {
                    out.push(b);
                }
            }
            State::Esc => match b {
                b'[' => {
                    self.seq.push(b);
                    self.state = State::Csi;
                }
                // Another ESC abandons this one.
                ESC => out.push(ESC),
                _ => {
                    out.append(&mut self.seq);
                    out.push(b);
                    self.state = match b {
                        b']' | b'P' | b'_' | b'^' | b'X' => State::Str,
                        _ => State::Ground,
                    };
                    if b == b'c' {
                        self.modes = Modes::default(); // RIS
                    }
                }
            },
            State::Csi => match b {
                0x40..=0x7e => {
                    self.seq.push(b);
                    let seq = std::mem::take(&mut self.seq);
                    out.extend(self.csi(seq));
                    self.state = State::Ground;
                }
                ESC => {
                    out.append(&mut self.seq);
                    self.seq.push(ESC);
                    self.state = State::Esc;
                }
                CAN | SUB => {
                    out.append(&mut self.seq);
                    out.push(b);
                    self.state = State::Ground;
                }
                _ => {
                    self.seq.push(b);
                    if self.seq.len() > MAX_CSI {
                        out.append(&mut self.seq);
                        self.state = State::CsiPass;
                    }
                }
            },
            State::CsiPass => match b {
                ESC => {
                    self.seq.push(ESC);
                    self.state = State::Esc;
                }
                _ => {
                    out.push(b);
                    if matches!(b, 0x40..=0x7e | CAN | SUB) {
                        self.state = State::Ground;
                    }
                }
            },
            State::Str => match b {
                ESC => self.state = State::StrEsc,
                _ => {
                    out.push(b);
                    if matches!(b, BEL | CAN | SUB) {
                        self.state = State::Ground;
                    }
                }
            },
            State::StrEsc => {
                if b == b'\\' {
                    out.extend([ESC, b]);
                    self.state = State::Ground;
                } else {
                    // The string ends here. Ended with ST, it stays ended whatever the sequence
                    // after it turns into.
                    out.extend(b"\x1b\\");
                    self.seq.push(ESC);
                    self.state = State::Esc;
                    self.step(b, out);
                }
            }
        }
    }

    /// One whole CSI: applies what it sets, and returns it without the reporting modes.
    fn csi(&mut self, seq: Vec<u8>) -> Vec<u8> {
        let body = &seq[2..seq.len() - 1];
        let last = seq[seq.len() - 1];
        let (prefix, rest) = match body.first() {
            Some(&p @ (b'?' | b'>' | b'<' | b'=')) => (p, &body[1..]),
            _ => (0, body),
        };
        let split = rest
            .iter()
            .position(|b| (0x20..=0x2f).contains(b))
            .unwrap_or(rest.len());
        let (digits, intermediates) = rest.split_at(split);
        let params: Vec<Option<u32>> = std::str::from_utf8(digits)
            .unwrap_or("")
            .split(';')
            .map(|p| p.split(':').next().and_then(|n| n.parse().ok()))
            .collect();
        match (prefix, intermediates, last) {
            (b'?', [], b'h' | b'l') => {
                let on = last == b'h';
                for mode in params.iter().flatten() {
                    self.modes.set_dec(*mode, on);
                }
                let kept: Vec<&str> = std::str::from_utf8(digits)
                    .unwrap_or("")
                    .split(';')
                    .filter(|p| !p.parse().is_ok_and(|m: u32| REPORTING.contains(&m)))
                    .collect();
                if kept.len() == params.len() {
                    seq
                } else if kept.is_empty() {
                    Vec::new()
                } else {
                    format!("\x1b[?{}{}", kept.join(";"), last as char).into_bytes()
                }
            }
            (b'>' | b'<' | b'=', [], b'u') => {
                self.modes.kitty(prefix, &params);
                seq
            }
            // DECSTR, the soft reset, turns application cursor keys off.
            (0, [b'!'], b'p') => {
                self.modes.app_cursor = false;
                seq
            }
            _ => seq,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_follow_the_output_and_reporting_never_reaches_the_browser() {
        let mut vt = Vt::default();
        let out = vt.feed(b"a\x1b[?1h\x1b[?2004h\x1b[?1000;1006hb\x1b[?1004h\x1b[?25;1003l");
        assert_eq!(out, b"a\x1b[?1h\x1b[?2004hb\x1b[?25l");
        assert!(vt.modes.app_cursor && vt.modes.bracketed_paste);
        assert_eq!(
            vt.modes.tracking,
            Tracking::Off,
            "1003 off turns tracking off"
        );
        assert_eq!(vt.modes.encoding, MouseEncoding::Sgr);
        vt.feed(b"\x1b[?1002h");
        assert_eq!(vt.modes.tracking, Tracking::Button);
        vt.feed(b"\x1bc");
        assert_eq!(vt.modes, Modes::default());
    }

    #[test]
    fn a_sequence_split_across_chunks_is_held_until_whole() {
        let mut vt = Vt::default();
        let mut out = vt.feed(b"x\x1b");
        out.extend(vt.feed(b"[?10"));
        assert_eq!(out, b"x");
        out.extend(vt.feed(b"00hy\x1b[3"));
        out.extend(vt.feed(b"1mz"));
        assert_eq!(out, b"xy\x1b[31mz");
        assert_eq!(vt.modes.tracking, Tracking::Normal);
    }

    #[test]
    fn strings_pass_through_and_hide_nothing() {
        let mut vt = Vt::default();
        let s = b"\x1b]0;a[?1000h\x07\x1b]2;u\x1b\\\x1bP+q544e\x1b\\";
        assert_eq!(vt.feed(s), s);
        assert_eq!(
            vt.modes.tracking,
            Tracking::Off,
            "inside a string it is no mode"
        );
        // ESC that is not ST ends the string and starts a sequence.
        let mut vt = Vt::default();
        assert_eq!(vt.feed(b"\x1b]0;t\x1b[?1000hz"), b"\x1b]0;t\x1b\\z");
        assert_eq!(vt.modes.tracking, Tracking::Normal);
    }

    #[test]
    fn cancelled_and_overlong_sequences_pass_on() {
        let mut vt = Vt::default();
        assert_eq!(vt.feed(b"\x1b[?10\x18a"), b"\x1b[?10\x18a");
        let long = format!("\x1b[{}m", "1;".repeat(40));
        assert_eq!(vt.feed(long.as_bytes()), long.as_bytes());
        assert_eq!(vt.feed(b"\x1b\x1b[?1h"), b"\x1b\x1b[?1h");
        assert!(vt.modes.app_cursor);
        vt.feed(b"\x1b[!p");
        assert!(!vt.modes.app_cursor, "DECSTR");
    }

    #[test]
    fn kitty_flags_are_a_stack_per_screen() {
        let mut vt = Vt::default();
        vt.feed(b"\x1b[>1u\x1b[>3u");
        assert_eq!(vt.modes.kitty_flags(), 3);
        vt.feed(b"\x1b[?1049h");
        assert_eq!(
            vt.modes.kitty_flags(),
            0,
            "the alternate screen has its own"
        );
        vt.feed(b"\x1b[=5u\x1b[=2;2u");
        assert_eq!(vt.modes.kitty_flags(), 7);
        vt.feed(b"\x1b[=1;3u");
        assert_eq!(vt.modes.kitty_flags(), 6);
        vt.feed(b"\x1b[?1049l\x1b[<u");
        assert_eq!(vt.modes.kitty_flags(), 1);
        vt.feed(b"\x1b[<5u");
        assert_eq!(vt.modes.kitty_flags(), 0);
    }

    #[test]
    fn the_first_paint_resets_then_replays_modes() {
        // What herdr-core's paint begins with (ghostty-vt's `paint`), then a program's own modes.
        let mut vt = Vt::default();
        vt.feed(b"\x1b[?1000h\x1b[>1u");
        let out = vt.feed(b"\x18\x1b[!p\x1b[?9l\x1b[?1000l\x1b[?1006l\x1b[?2004l\x1b[?1l\x1b[=0;1u\x1b[H\x1b[2J\x1b[?2004h");
        assert_eq!(
            out,
            b"\x18\x1b[!p\x1b[?2004l\x1b[?1l\x1b[=0;1u\x1b[H\x1b[2J\x1b[?2004h"
        );
        assert_eq!(vt.modes.tracking, Tracking::Off);
        assert_eq!(vt.modes.kitty_flags(), 0);
        assert!(vt.modes.bracketed_paste);
    }
}
