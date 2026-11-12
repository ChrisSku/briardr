// Input as the bytes a terminal sends for it, for the program's modes (`vt.rs`): herdr-core takes
// bytes and never encodes keys. Key names are the ones Briardr sends Herdr TUI (`ctrl+shift+tab`,
// `up`, `f5`, `super+a`): modifiers joined with `+`, then one key. The bytes are Ghostty's (the
// Herdr app's terminal, whose encoder `hc keys` uses), so a key does the same from the phone as at
// the desk. Where Ghostty sends nothing for lack of the typed text, a key gives the text a US
// keyboard types (Shift+1 is `!`) and legacy terminals' Control characters. Without the kitty
// keyboard protocol, Command reaches only keys whose sequences carry modifiers; a character with
// Command is dropped, as Herdr TUI drops it.
use super::vt::{Modes, MouseEncoding, Tracking};
use crate::herdr::stream::{Direction, Input, MouseAction, MouseButton};

const SHIFT: u32 = 1;
const ALT: u32 = 2;
const CTRL: u32 = 4;
const SUPER: u32 = 8;
/// Kitty keyboard flags: disambiguate escape codes, report all keys as escape codes.
const DISAMBIGUATE: u32 = 1;
const ALL_KEYS: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Char(char),
    Enter,
    Esc,
    Tab,
    Backspace,
    /// The final letter of an arrow's sequence.
    Arrow(u8),
    F(u8),
}

fn parse(name: &str) -> Option<(u32, Key)> {
    let mut parts: Vec<&str> = name.split('+').collect();
    let key = parts.pop()?;
    let mut mods = 0;
    for m in parts {
        mods |= match m {
            "shift" => SHIFT,
            "alt" | "meta" => ALT,
            "ctrl" | "control" => CTRL,
            "super" | "cmd" => SUPER,
            _ => return None,
        };
    }
    let key = match key {
        "enter" => Key::Enter,
        "esc" | "escape" => Key::Esc,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "up" => Key::Arrow(b'A'),
        "down" => Key::Arrow(b'B'),
        "right" => Key::Arrow(b'C'),
        "left" => Key::Arrow(b'D'),
        "space" => Key::Char(' '),
        "minus" => Key::Char('-'),
        "plus" => Key::Char('+'),
        f if f.len() > 1 && f.starts_with('f') => {
            Key::F(f[1..].parse().ok().filter(|n| (1..=12).contains(n))?)
        }
        c => {
            let mut cs = c.chars();
            match (cs.next(), cs.next()) {
                (Some(c), None) => Key::Char(c),
                _ => return None,
            }
        }
    };
    Some((mods, key))
}

/// The character Shift gives on a US keyboard.
fn shifted(c: char) -> char {
    const PAIRS: &str = "1!2@3#4$5%6^7&8*9(0)-_=+[{]}\\|;:'\",<.>/?`~";
    if c.is_ascii_lowercase() {
        return c.to_ascii_uppercase();
    }
    let cs: Vec<char> = PAIRS.chars().collect();
    cs.chunks(2).find(|p| p[0] == c).map_or(c, |p| p[1])
}

/// Control and a character, as legacy terminals send it.
fn control(c: char) -> char {
    match c {
        'a'..='z' => char::from(c as u8 - b'a' + 1),
        '@' | '2' | ' ' => '\0',
        '[' | '3' => '\x1b',
        '\\' | '4' => '\x1c',
        ']' | '5' => '\x1d',
        '6' => '\x1e',
        '/' | '7' | '-' => '\x1f',
        '8' => '\x7f',
        _ => c,
    }
}

/// The xterm modifier parameter: 1 plus the modifier bits.
fn param(mods: u32) -> u32 {
    1 + mods
}

/// `CSI n u`, or `CSI n ; m u` with modifiers.
fn csi_u(code: u32, mods: u32) -> String {
    match mods {
        0 => format!("\x1b[{code}u"),
        m => format!("\x1b[{code};{}u", param(m)),
    }
}

/// `CSI n ~`, or `CSI n ; m ~` with modifiers.
fn tilde(n: u32, mods: u32) -> String {
    match mods {
        0 => format!("\x1b[{n}~"),
        m => format!("\x1b[{n};{}~", param(m)),
    }
}

/// Arrows and F1–F4: `SS3 x` where `ss3`, else `CSI x`, and `CSI 1 ; m x` with modifiers.
fn letter(x: u8, mods: u32, ss3: bool) -> String {
    let x = x as char;
    match mods {
        0 if ss3 => format!("\x1bO{x}"),
        0 => format!("\x1b[{x}"),
        m => format!("\x1b[1;{}{x}", param(m)),
    }
}

/// F5–F12 as their `CSI n ~` numbers.
fn f_number(n: u8) -> u32 {
    [15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)]
}

/// F1–F4 are `SS3 x` unmodified (kitty: `CSI x`); `CSI 1;m R` would read as a cursor position
/// report, so F3 is `CSI 13 ~` where it isn't SS3.
fn function(n: u8, mods: u32, kitty: bool) -> String {
    match n {
        3 if kitty || mods != 0 => tilde(13, mods),
        1..=4 => letter(b"PQRS"[usize::from(n - 1)], mods, !kitty),
        _ => tilde(f_number(n), mods),
    }
}

/// xterm's `CSI 27 ; m ; code ~`, for a key with modifiers legacy bytes can't carry.
fn other(code: u32, mods: u32) -> String {
    format!("\x1b[27;{};{code}~", param(mods))
}

/// One named key, or None when the program can't be sent it.
pub fn key(name: &str, modes: &Modes) -> Option<Vec<u8>> {
    let (mods, key) = parse(name)?;
    let flags = modes.kitty_flags();
    let out = if flags & (DISAMBIGUATE | ALL_KEYS) != 0 {
        kitty_key(key, mods, flags & ALL_KEYS != 0)
    } else {
        legacy_key(key, mods, modes.app_cursor)?
    };
    Some(out.into_bytes())
}

/// With the kitty protocol, cursor keys ignore application mode.
fn kitty_key(key: Key, mods: u32, all: bool) -> String {
    match key {
        Key::Char(c) if !all && mods & !SHIFT == 0 => {
            (if mods & SHIFT != 0 { shifted(c) } else { c }).to_string()
        }
        Key::Char(c) => csi_u(u32::from(c), mods),
        Key::Enter | Key::Tab | Key::Backspace if !all && mods == 0 => {
            legacy_key(key, 0, false).unwrap_or_default()
        }
        Key::Enter => csi_u(13, mods),
        Key::Tab => csi_u(9, mods),
        Key::Backspace => csi_u(127, mods),
        Key::Esc => csi_u(27, mods),
        Key::Arrow(x) => letter(x, mods, false),
        Key::F(n) => function(n, mods, true),
    }
}

fn legacy_key(key: Key, mods: u32, app_cursor: bool) -> Option<String> {
    let alt = if mods & ALT != 0 { "\x1b" } else { "" };
    Some(match key {
        Key::Char(_) | Key::Backspace if mods & SUPER != 0 => return None,
        Key::Char(c) => {
            let c = match (mods & CTRL != 0, mods & SHIFT != 0) {
                (true, _) => control(c),
                (false, true) => shifted(c),
                _ => c,
            };
            format!("{alt}{c}")
        }
        Key::Enter if mods & !ALT == 0 => format!("{alt}\r"),
        Key::Enter => other(13, mods),
        Key::Tab if mods == SHIFT => "\x1b[Z".to_string(),
        Key::Tab if mods & !ALT == 0 => format!("{alt}\t"),
        Key::Tab => other(9, mods),
        Key::Esc if mods & !ALT == 0 => format!("{alt}\x1b"),
        Key::Esc => other(27, mods),
        Key::Backspace if mods & CTRL != 0 => format!("{alt}\x08"),
        Key::Backspace => format!("{alt}\x7f"),
        Key::Arrow(x) => letter(x, mods, app_cursor),
        Key::F(n) => function(n, mods, false),
    })
}

/// Bytes for one input. A key that can't be sent is left out.
pub fn input(input: &Input, modes: &Modes) -> Vec<u8> {
    match input {
        Input::Text(text) => text.as_bytes().to_vec(),
        Input::Keys(keys) => keys
            .iter()
            .filter_map(|k| key(k, modes))
            .flatten()
            .collect(),
        Input::Raw(seq) => seq.as_bytes().to_vec(),
        // The text is sanitized, so it can't end the bracket itself.
        Input::Paste { text, bracketed } if *bracketed && modes.bracketed_paste => {
            [b"\x1b[200~", text.as_bytes(), b"\x1b[201~"].concat()
        }
        Input::Paste { text, .. } => text.as_bytes().to_vec(),
    }
}

/// One mouse event at a cell (0-based), for a program that asked for it; `modifiers` are Briardr's
/// (shift 1, ctrl 2, alt 4).
pub fn mouse(
    action: MouseAction,
    button: MouseButton,
    column: i64,
    row: i64,
    modifiers: i64,
    modes: &Modes,
) -> Option<Vec<u8>> {
    let wanted = match modes.tracking {
        Tracking::Off => false,
        Tracking::X10 => action == MouseAction::Down,
        Tracking::Normal => matches!(action, MouseAction::Down | MouseAction::Up),
        Tracking::Button => action != MouseAction::Move,
        Tracking::Any => true,
    };
    if !wanted {
        return None;
    }
    let base = match (action, button) {
        (MouseAction::Move, _) => 3,
        (_, MouseButton::Left) => 0,
        (_, MouseButton::Middle) => 1,
        (_, MouseButton::Right) => 2,
    };
    let motion = if matches!(action, MouseAction::Drag | MouseAction::Move) {
        32
    } else {
        0
    };
    let mods = if modes.tracking == Tracking::X10 {
        0
    } else {
        (if modifiers & 1 != 0 { 4 } else { 0 })
            | (if modifiers & 4 != 0 { 8 } else { 0 })
            | (if modifiers & 2 != 0 { 16 } else { 0 })
    };
    report(
        base + motion + mods,
        action == MouseAction::Up,
        column,
        row,
        modes.encoding,
    )
}

/// A scroll-wheel step at a cell: a wheel event for a program that takes the mouse, an arrow key on
/// the alternate screen (alternate scroll), and nothing on the main screen, whose history the
/// browser's terminal scrolls itself.
pub fn scroll(direction: Direction, column: i64, row: i64, modes: &Modes) -> Option<Vec<u8>> {
    if modes.wants_mouse() {
        let button = if direction == Direction::Up { 64 } else { 65 };
        return report(button, false, column, row, modes.encoding);
    }
    if modes.alt_screen {
        return key(
            if direction == Direction::Up {
                "up"
            } else {
                "down"
            },
            modes,
        );
    }
    None
}

fn report(
    code: u32,
    release: bool,
    column: i64,
    row: i64,
    encoding: MouseEncoding,
) -> Option<Vec<u8>> {
    let (x, y) = (column.max(0) as u32 + 1, row.max(0) as u32 + 1);
    // Outside SGR a release can't say which button: it is button 3.
    let legacy = if release { (code & !3) | 3 } else { code };
    Some(match encoding {
        MouseEncoding::Sgr => {
            format!("\x1b[<{code};{x};{y}{}", if release { 'm' } else { 'M' }).into_bytes()
        }
        MouseEncoding::Urxvt => format!("\x1b[{};{x};{y}M", legacy + 32).into_bytes(),
        MouseEncoding::Utf8 => {
            let mut s = String::from("\x1b[M");
            for n in [legacy + 32, x + 32, y + 32] {
                s.push(char::from_u32(n).filter(|_| n < 2048)?);
            }
            s.into_bytes()
        }
        MouseEncoding::X10 => {
            let bytes = [legacy + 32, x + 32, y + 32];
            if bytes.iter().any(|b| *b > 255) {
                return None;
            }
            [b"\x1b[M".as_slice(), &bytes.map(|b| b as u8)].concat()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::core::vt::Vt;

    fn modes(output: &[u8]) -> Modes {
        let mut vt = Vt::default();
        vt.feed(output);
        vt.modes
    }

    fn keys(names: &[&str], modes: &Modes) -> Vec<String> {
        names
            .iter()
            .map(|k| key(k, modes).map_or("-".into(), |b| String::from_utf8(b).unwrap()))
            .collect()
    }

    /// What Ghostty's encoder sent for each key (through `hc keys`, herdr-core 0.1.20), in hex, once
    /// a program had turned on the modes given first: none, application cursor keys, kitty flags 1
    /// and 9, and both.
    const GHOSTTY: &[(&str, &str)] = &[
        (
            "",
            "a=61 shift+a=41 ctrl+c=03 alt+b=1b62 ctrl+alt+x=1b18 ctrl+space=00 enter=0d \
             alt+enter=1b0d shift+enter=1b5b32373b323b31337e ctrl+enter=1b5b32373b353b31337e \
             tab=09 shift+tab=1b5b5a ctrl+shift+tab=1b5b32373b363b397e backspace=7f \
             ctrl+backspace=08 alt+backspace=1b7f esc=1b alt+esc=1b1b space=20 up=1b5b41 \
             shift+left=1b5b313b3244 ctrl+alt+right=1b5b313b3743 super+up=1b5b313b3941 \
             f1=1b4f50 f3=1b4f52 shift+f3=1b5b31333b327e ctrl+f3=1b5b31333b357e f5=1b5b31357e \
             ctrl+f12=1b5b32343b357e ctrl+/=1f ctrl+2=00 alt+.=1b2e \
             ctrl+tab=1b5b32373b353b397e alt+tab=1b09 shift+backspace=7f \
             ctrl+alt+enter=1b5b32373b373b31337e alt+shift+enter=1b5b32373b343b31337e \
             shift+space=20 alt+space=1b20 ctrl+alt+space=1b00 ctrl+1=31 \
             shift+esc=1b5b32373b323b32377e ctrl+esc=1b5b32373b353b32377e \
             ctrl+alt+backspace=1b08 alt+up=1b5b313b3341 ctrl+up=1b5b313b3541 \
             shift+f1=1b5b313b3250 super+f5=1b5b31353b397e shift+up=1b5b313b3241 f2=1b4f51 \
             f4=1b4f53 ctrl+f1=1b5b313b3550 ctrl+8=7f ctrl+6=1e ctrl+\\=1c ctrl+]=1d \
             alt+shift+tab=1b5b32373b343b397e super+enter=1b5b32373b393b31337e \
             super+tab=1b5b32373b393b397e super+esc=1b5b32373b393b32377e",
        ),
        (
            "\x1b[?1h",
            "a=61 shift+a=41 ctrl+c=03 alt+b=1b62 ctrl+alt+x=1b18 ctrl+space=00 enter=0d \
             alt+enter=1b0d shift+enter=1b5b32373b323b31337e ctrl+enter=1b5b32373b353b31337e \
             tab=09 shift+tab=1b5b5a ctrl+shift+tab=1b5b32373b363b397e backspace=7f \
             ctrl+backspace=08 alt+backspace=1b7f esc=1b alt+esc=1b1b space=20 up=1b4f41 \
             shift+left=1b5b313b3244 ctrl+alt+right=1b5b313b3743 super+up=1b5b313b3941 \
             f1=1b4f50 f3=1b4f52 shift+f3=1b5b31333b327e ctrl+f3=1b5b31333b357e f5=1b5b31357e \
             ctrl+f12=1b5b32343b357e ctrl+/=1f ctrl+2=00 alt+.=1b2e",
        ),
        (
            "\x1b[>1u",
            "a=61 ctrl+c=1b5b39393b3575 alt+b=1b5b39383b3375 ctrl+alt+x=1b5b3132303b3775 \
             ctrl+[=1b5b39313b3575 ctrl+space=1b5b33323b3575 super+a=1b5b39373b3975 enter=0d \
             alt+enter=1b5b31333b3375 shift+enter=1b5b31333b3275 ctrl+enter=1b5b31333b3575 \
             tab=09 shift+tab=1b5b393b3275 ctrl+shift+tab=1b5b393b3675 backspace=7f \
             ctrl+backspace=1b5b3132373b3575 alt+backspace=1b5b3132373b3375 esc=1b5b323775 \
             alt+esc=1b5b32373b3375 space=20 up=1b5b41 shift+left=1b5b313b3244 \
             ctrl+alt+right=1b5b313b3743 super+up=1b5b313b3941 f1=1b5b50 f3=1b5b31337e \
             shift+f3=1b5b31333b327e ctrl+f3=1b5b31333b357e f5=1b5b31357e \
             ctrl+f12=1b5b32343b357e ctrl+/=1b5b34373b3575 ctrl+2=1b5b35303b3575 \
             alt+.=1b5b34363b3375 super+`=1b5b39363b3975 ctrl+tab=1b5b393b3575 \
             alt+tab=1b5b393b3375 shift+backspace=1b5b3132373b3275 \
             ctrl+alt+enter=1b5b31333b3775 alt+shift+enter=1b5b31333b3475 \
             alt+space=1b5b33323b3375 ctrl+alt+space=1b5b33323b3775 \
             ctrl+shift+a=1b5b39373b3675 alt+shift+a=1b5b39373b3475 ctrl+1=1b5b34393b3575 \
             ctrl+.=1b5b34363b3575 ctrl+;=1b5b35393b3575 ctrl+-=1b5b34353b3575 \
             ctrl+==1b5b36313b3575 shift+esc=1b5b32373b3275 ctrl+esc=1b5b32373b3575 \
             ctrl+alt+backspace=1b5b3132373b3775 alt+up=1b5b313b3341 ctrl+up=1b5b313b3541 \
             shift+f1=1b5b313b3250 super+f5=1b5b31353b397e shift+up=1b5b313b3241 f2=1b5b51 \
             f4=1b5b53 ctrl+f1=1b5b313b3550 ctrl+8=1b5b35363b3575 ctrl+6=1b5b35343b3575 \
             ctrl+\\=1b5b39323b3575 ctrl+]=1b5b39333b3575 alt+shift+tab=1b5b393b3475",
        ),
        (
            "\x1b[>9u",
            "a=1b5b393775 shift+a=1b5b39373b3275 ctrl+c=1b5b39393b3575 alt+b=1b5b39383b3375 \
             ctrl+alt+x=1b5b3132303b3775 ctrl+[=1b5b39313b3575 ctrl+space=1b5b33323b3575 \
             shift+1=1b5b34393b3275 super+a=1b5b39373b3975 enter=1b5b313375 \
             alt+enter=1b5b31333b3375 shift+enter=1b5b31333b3275 ctrl+enter=1b5b31333b3575 \
             tab=1b5b3975 shift+tab=1b5b393b3275 ctrl+shift+tab=1b5b393b3675 \
             backspace=1b5b31323775 ctrl+backspace=1b5b3132373b3575 \
             alt+backspace=1b5b3132373b3375 esc=1b5b323775 alt+esc=1b5b32373b3375 \
             space=1b5b333275 up=1b5b41 shift+left=1b5b313b3244 ctrl+alt+right=1b5b313b3743 \
             super+up=1b5b313b3941 f1=1b5b50 f3=1b5b31337e shift+f3=1b5b31333b327e \
             ctrl+f3=1b5b31333b357e f5=1b5b31357e ctrl+f12=1b5b32343b357e \
             ctrl+/=1b5b34373b3575 ctrl+2=1b5b35303b3575 alt+.=1b5b34363b3375 \
             super+`=1b5b39363b3975",
        ),
        (
            "\x1b[?1h\x1b[>1u",
            "up=1b5b41 ctrl+up=1b5b313b3541 f1=1b5b50 f3=1b5b31337e enter=0d \
             super+enter=1b5b31333b3975 super+tab=1b5b393b3975 super+esc=1b5b32373b3975",
        ),
    ];

    #[test]
    fn keys_are_ghosttys() {
        for (output, cases) in GHOSTTY {
            let m = modes(output.as_bytes());
            for case in cases.split(' ') {
                let (name, hex) = case.rsplit_once('=').unwrap();
                let got =
                    key(name, &m).map(|b| b.iter().map(|x| format!("{x:02x}")).collect::<String>());
                assert_eq!(got.as_deref(), Some(hex), "{name} after {output:?}");
            }
        }
    }

    #[test]
    fn keys_type_what_a_keyboard_types_where_ghostty_has_no_text() {
        let legacy = Modes::default();
        assert_eq!(
            keys(
                &[
                    "shift+1",
                    "alt+shift+a",
                    "ctrl+shift+a",
                    "ctrl+[",
                    "ctrl+minus",
                    "ctrl+.",
                    "super+a",
                    "super+backspace"
                ],
                &legacy
            ),
            ["!", "\x1bA", "\x01", "\x1b", "\x1f", ".", "-", "-"]
        );
        assert_eq!(
            keys(
                &["shift+a", "shift+1", "shift+space", "plus"],
                &modes(b"\x1b[>1u")
            ),
            ["A", "!", " ", "+"]
        );
        assert_eq!(
            keys(&["super+a"], &modes(b"\x1b[>1u\x1b[<u")),
            ["-"],
            "popped back to legacy"
        );
        assert_eq!(
            keys(&["nope", "ctrl+", "f13", "hyper+a"], &legacy),
            ["-", "-", "-", "-"]
        );
    }

    #[test]
    fn pastes_are_bracketed_only_when_the_program_asked() {
        let paste = Input::Paste {
            text: "x y".into(),
            bracketed: true,
        };
        assert_eq!(input(&paste, &Modes::default()), b"x y");
        assert_eq!(
            input(&paste, &modes(b"\x1b[?2004h")),
            b"\x1b[200~x y\x1b[201~"
        );
        let plain = Input::Paste {
            text: "x".into(),
            bracketed: false,
        };
        assert_eq!(input(&plain, &modes(b"\x1b[?2004h")), b"x");
        let both = Input::Keys(vec!["a".into(), "super+b".into(), "enter".into()]);
        assert_eq!(input(&both, &Modes::default()), b"a\r");
    }

    #[test]
    fn mouse_reports_follow_tracking_and_encoding() {
        use MouseAction::*;
        use MouseButton::*;
        assert_eq!(mouse(Down, Left, 0, 0, 0, &Modes::default()), None);
        let sgr = modes(b"\x1b[?1002h\x1b[?1006h");
        assert_eq!(mouse(Down, Left, 4, 2, 0, &sgr).unwrap(), b"\x1b[<0;5;3M");
        assert_eq!(mouse(Up, Left, 4, 2, 1, &sgr).unwrap(), b"\x1b[<4;5;3m");
        assert_eq!(mouse(Drag, Right, 0, 0, 2, &sgr).unwrap(), b"\x1b[<50;1;1M");
        assert_eq!(
            mouse(Move, Left, 0, 0, 0, &sgr),
            None,
            "1002 doesn't report movement"
        );
        let x10 = modes(b"\x1b[?1000h");
        assert_eq!(mouse(Up, Middle, 1, 1, 0, &x10).unwrap(), b"\x1b[M#\"\"");
        assert_eq!(
            mouse(Down, Left, 300, 0, 0, &x10),
            None,
            "too far right to say"
        );
        let any = modes(b"\x1b[?1003h\x1b[?1015h");
        assert_eq!(mouse(Move, Left, 0, 0, 4, &any).unwrap(), b"\x1b[75;1;1M");
    }

    #[test]
    fn scrolling_is_the_wheel_arrows_or_nothing() {
        let sgr = modes(b"\x1b[?1000h\x1b[?1006h");
        assert_eq!(scroll(Direction::Up, 2, 3, &sgr).unwrap(), b"\x1b[<64;3;4M");
        assert_eq!(
            scroll(Direction::Down, 2, 3, &modes(b"\x1b[?1049h\x1b[?1h")).unwrap(),
            b"\x1bOB"
        );
        assert_eq!(scroll(Direction::Up, 0, 0, &Modes::default()), None);
    }
}
