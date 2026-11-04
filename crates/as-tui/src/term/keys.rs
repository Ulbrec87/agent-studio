//! crossterm key events -> the bytes an xterm-compatible terminal would send to the application.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// xterm modifier parameter (`1 + shift + 2*alt + 4*ctrl`), `None` without modifiers.
fn modifier_param(mods: KeyModifiers) -> Option<u8> {
    let mut m = 0u8;
    if mods.contains(KeyModifiers::SHIFT) {
        m |= 1;
    }
    if mods.contains(KeyModifiers::ALT) {
        m |= 2;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        m |= 4;
    }
    (m != 0).then_some(m + 1)
}

/// `ESC [ <final>` / `ESC O <final>`, or `ESC [ 1 ; <mod> <final>` with modifiers.
fn csi_letter(final_byte: u8, mods: KeyModifiers, application: bool) -> Vec<u8> {
    match modifier_param(mods) {
        Some(m) => format!("\x1b[1;{m}{}", final_byte as char).into_bytes(),
        None if application => vec![0x1b, b'O', final_byte],
        None => vec![0x1b, b'[', final_byte],
    }
}

/// `ESC [ <n> ~` or `ESC [ <n> ; <mod> ~`.
fn csi_tilde(n: u8, mods: KeyModifiers) -> Vec<u8> {
    match modifier_param(mods) {
        Some(m) => format!("\x1b[{n};{m}~").into_bytes(),
        None => format!("\x1b[{n}~").into_bytes(),
    }
}

fn function_key(n: u8, mods: KeyModifiers) -> Vec<u8> {
    match n {
        1..=4 => {
            let letter = b"PQRS"[usize::from(n) - 1];
            match modifier_param(mods) {
                Some(m) => format!("\x1b[1;{m}{}", letter as char).into_bytes(),
                None => vec![0x1b, b'O', letter],
            }
        }
        5 => csi_tilde(15, mods),
        6 => csi_tilde(17, mods),
        7 => csi_tilde(18, mods),
        8 => csi_tilde(19, mods),
        9 => csi_tilde(20, mods),
        10 => csi_tilde(21, mods),
        11 => csi_tilde(23, mods),
        12 => csi_tilde(24, mods),
        _ => Vec::new(),
    }
}

/// The control byte of `Ctrl+<c>`, when one exists.
fn control_byte(c: char) -> Option<u8> {
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        '@' | ' ' | '2' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '7' | '/' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

/// Bytes for a key press. `application_cursor` is DECCKM (`Screen::application_cursor`).
/// Releases and unmapped keys give an empty vector.
pub fn encode_key(key: KeyEvent, application_cursor: bool) -> Vec<u8> {
    if key.kind == KeyEventKind::Release {
        return Vec::new();
    }
    let mods = key.modifiers;
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let alt = mods.contains(KeyModifiers::ALT);
    let shift = mods.contains(KeyModifiers::SHIFT);
    // Modifiers that still need to be expressed in a CSI parameter (alt is an ESC prefix only for
    // characters and a few keys, but a parameter for the cursor/function keys).
    let csi_mods = mods & (KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL);

    let mut out = match key.code {
        KeyCode::Char(c) => {
            let mut bytes = Vec::new();
            if ctrl {
                match control_byte(c) {
                    Some(b) => bytes.push(b),
                    None => return Vec::new(),
                }
            } else {
                let mut buf = [0u8; 4];
                bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
            if alt {
                bytes.insert(0, 0x1b);
            }
            return bytes;
        }
        KeyCode::Enter => {
            if shift && !alt && !ctrl {
                // Newline without submitting (Claude Code / Codex treat LF as "insert newline").
                vec![b'\n']
            } else {
                vec![b'\r']
            }
        }
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => {
            if ctrl {
                vec![0x08]
            } else {
                vec![0x7f]
            }
        }
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => csi_letter(b'A', csi_mods, application_cursor),
        KeyCode::Down => csi_letter(b'B', csi_mods, application_cursor),
        KeyCode::Right => csi_letter(b'C', csi_mods, application_cursor),
        KeyCode::Left => csi_letter(b'D', csi_mods, application_cursor),
        KeyCode::Home => csi_letter(b'H', csi_mods, application_cursor),
        KeyCode::End => csi_letter(b'F', csi_mods, application_cursor),
        KeyCode::Insert => csi_tilde(2, csi_mods),
        KeyCode::Delete => csi_tilde(3, csi_mods),
        KeyCode::PageUp => csi_tilde(5, csi_mods),
        KeyCode::PageDown => csi_tilde(6, csi_mods),
        KeyCode::F(n) => function_key(n, csi_mods),
        _ => return Vec::new(),
    };
    // Alt on the simple keys is an ESC prefix; the CSI forms already carry it as a parameter.
    let is_csi = out.len() > 1 && out[0] == 0x1b && (out[1] == b'[' || out[1] == b'O');
    if alt && !is_csi && !out.is_empty() {
        out.insert(0, 0x1b);
    }
    out
}

/// Bytes for pasted text: wrapped in `ESC[200~ ... ESC[201~` when the application enabled bracketed
/// paste, otherwise newlines become `\r` (what typing Enter would send).
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        // A paste must not be able to end the bracket itself.
        let safe = text.replace("\x1b[201~", "");
        let mut out = b"\x1b[200~".to_vec();
        out.extend_from_slice(safe.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    fn enc(code: KeyCode, mods: KeyModifiers) -> Vec<u8> {
        encode_key(key(code, mods), false)
    }

    const NONE: KeyModifiers = KeyModifiers::NONE;
    const CTRL: KeyModifiers = KeyModifiers::CONTROL;
    const ALT: KeyModifiers = KeyModifiers::ALT;
    const SHIFT: KeyModifiers = KeyModifiers::SHIFT;

    #[test]
    fn printable_characters_are_utf8() {
        assert_eq!(enc(KeyCode::Char('a'), NONE), b"a");
        assert_eq!(enc(KeyCode::Char('A'), SHIFT), b"A");
        assert_eq!(enc(KeyCode::Char('é'), NONE), "é".as_bytes());
        assert_eq!(enc(KeyCode::Char('日'), NONE), "日".as_bytes());
        assert_eq!(enc(KeyCode::Char('😀'), NONE), "😀".as_bytes());
    }

    #[test]
    fn control_letters() {
        assert_eq!(enc(KeyCode::Char('a'), CTRL), [1]);
        assert_eq!(enc(KeyCode::Char('c'), CTRL), [3]);
        assert_eq!(enc(KeyCode::Char('z'), CTRL), [26]);
        assert_eq!(enc(KeyCode::Char('C'), CTRL | SHIFT), [3]);
        assert_eq!(enc(KeyCode::Char('['), CTRL), [0x1b]);
        assert_eq!(enc(KeyCode::Char('\\'), CTRL), [0x1c]);
        assert_eq!(enc(KeyCode::Char(' '), CTRL), [0]);
        assert_eq!(enc(KeyCode::Char('_'), CTRL), [0x1f]);
        assert_eq!(enc(KeyCode::Char('?'), CTRL), [0x7f]);
        assert!(enc(KeyCode::Char('é'), CTRL).is_empty());
    }

    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(enc(KeyCode::Char('b'), ALT), b"\x1bb");
        assert_eq!(enc(KeyCode::Char('f'), ALT | CTRL), b"\x1b\x06");
        assert_eq!(enc(KeyCode::Enter, ALT), b"\x1b\r");
        assert_eq!(enc(KeyCode::Backspace, ALT), b"\x1b\x7f");
        assert_eq!(enc(KeyCode::Esc, ALT), b"\x1b\x1b");
    }

    #[test]
    fn simple_keys() {
        assert_eq!(enc(KeyCode::Enter, NONE), b"\r");
        assert_eq!(enc(KeyCode::Enter, SHIFT), b"\n");
        assert_eq!(enc(KeyCode::Tab, NONE), b"\t");
        assert_eq!(enc(KeyCode::BackTab, SHIFT), b"\x1b[Z");
        assert_eq!(enc(KeyCode::Backspace, NONE), [0x7f]);
        assert_eq!(enc(KeyCode::Backspace, CTRL), [0x08]);
        assert_eq!(enc(KeyCode::Esc, NONE), [0x1b]);
    }

    #[test]
    fn arrows_respect_application_cursor_mode() {
        let up = key(KeyCode::Up, NONE);
        assert_eq!(encode_key(up, false), b"\x1b[A");
        assert_eq!(encode_key(up, true), b"\x1bOA");
        assert_eq!(enc(KeyCode::Down, NONE), b"\x1b[B");
        assert_eq!(enc(KeyCode::Right, NONE), b"\x1b[C");
        assert_eq!(enc(KeyCode::Left, NONE), b"\x1b[D");
        assert_eq!(encode_key(key(KeyCode::Home, NONE), false), b"\x1b[H");
        assert_eq!(encode_key(key(KeyCode::End, NONE), true), b"\x1bOF");
    }

    #[test]
    fn modified_arrows_ignore_application_mode() {
        assert_eq!(
            encode_key(key(KeyCode::Up, CTRL), true),
            b"\x1b[1;5A".to_vec()
        );
        assert_eq!(enc(KeyCode::Right, SHIFT), b"\x1b[1;2C");
        assert_eq!(enc(KeyCode::Left, ALT), b"\x1b[1;3D");
        assert_eq!(enc(KeyCode::Left, CTRL | SHIFT), b"\x1b[1;6D");
        assert_eq!(enc(KeyCode::Home, CTRL), b"\x1b[1;5H");
    }

    #[test]
    fn editing_keys() {
        assert_eq!(enc(KeyCode::Insert, NONE), b"\x1b[2~");
        assert_eq!(enc(KeyCode::Delete, NONE), b"\x1b[3~");
        assert_eq!(enc(KeyCode::PageUp, NONE), b"\x1b[5~");
        assert_eq!(enc(KeyCode::PageDown, NONE), b"\x1b[6~");
        assert_eq!(enc(KeyCode::Delete, CTRL), b"\x1b[3;5~");
        assert_eq!(enc(KeyCode::PageUp, ALT), b"\x1b[5;3~");
    }

    #[test]
    fn function_keys() {
        assert_eq!(enc(KeyCode::F(1), NONE), b"\x1bOP");
        assert_eq!(enc(KeyCode::F(4), NONE), b"\x1bOS");
        assert_eq!(enc(KeyCode::F(5), NONE), b"\x1b[15~");
        assert_eq!(enc(KeyCode::F(10), NONE), b"\x1b[21~");
        assert_eq!(enc(KeyCode::F(12), NONE), b"\x1b[24~");
        assert_eq!(enc(KeyCode::F(2), CTRL), b"\x1b[1;5Q");
        assert_eq!(enc(KeyCode::F(6), SHIFT), b"\x1b[17;2~");
        assert!(enc(KeyCode::F(13), NONE).is_empty());
    }

    #[test]
    fn releases_and_unknown_keys_are_ignored() {
        let mut release = key(KeyCode::Char('a'), NONE);
        release.kind = KeyEventKind::Release;
        assert!(encode_key(release, false).is_empty());
        assert!(enc(KeyCode::CapsLock, NONE).is_empty());
        let mut repeat = key(KeyCode::Char('a'), NONE);
        repeat.kind = KeyEventKind::Repeat;
        assert_eq!(encode_key(repeat, false), b"a");
    }

    #[test]
    fn paste_is_bracketed_when_enabled() {
        assert_eq!(encode_paste("hi\nyo", true), b"\x1b[200~hi\nyo\x1b[201~");
        assert_eq!(encode_paste("hi\r\nyo\n", false), b"hi\ryo\r");
        assert_eq!(
            encode_paste("a\x1b[201~b", true),
            b"\x1b[200~ab\x1b[201~".to_vec()
        );
    }
}
