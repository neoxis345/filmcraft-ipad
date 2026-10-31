//! Hardware keyboards (Magic Keyboard, any Bluetooth or USB keyboard) for FilmCraft's shortcuts.
//!
//! UIKit reports a key as its USB HID usage (the physical key, laid out as on a US keyboard) with
//! the characters it types. FilmCraft's shortcuts are logical keys ("Q" is the key labelled Q), so
//! on a French AZERTY keyboard the key labelled A must be `Key::A` even though its HID usage is the
//! US Q position: the logical key comes from the characters when they name one, and from the HID
//! usage otherwise (arrows, Return, F-keys…). The physical key is always the HID usage.
//!
//! Modifiers follow the Mac (iPad keyboards have Command): Command is `mac_cmd` and `command`.

use egui::{Key, Modifiers};

/// UIKeyModifierFlags bits.
pub const MOD_CAPS: u64 = 1 << 16;
pub const MOD_SHIFT: u64 = 1 << 17;
pub const MOD_CONTROL: u64 = 1 << 18;
pub const MOD_ALT: u64 = 1 << 19;
pub const MOD_COMMAND: u64 = 1 << 20;

/// The egui modifiers for UIKit modifier flags (Mac conventions).
pub fn modifiers(flags: u64) -> Modifiers {
    let cmd = flags & MOD_COMMAND != 0;
    Modifiers { alt: flags & MOD_ALT != 0, ctrl: flags & MOD_CONTROL != 0, shift: flags & MOD_SHIFT != 0, mac_cmd: cmd, command: cmd }
}

/// Whether the HID usage is a modifier key itself (no key event for egui).
pub fn is_modifier(usage: u32) -> bool {
    (224..=231).contains(&usage) || usage == 57
}

/// The key at a HID usage (USB HID usage table, keyboard page), as on a US layout.
pub fn physical(usage: u32) -> Option<Key> {
    use Key::*;
    const LETTERS: [Key; 26] = [A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z];
    const DIGITS: [Key; 10] = [Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9, Num0];
    const FKEYS: [Key; 12] = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12];
    const FKEYS_HIGH: [Key; 12] = [F13, F14, F15, F16, F17, F18, F19, F20, F21, F22, F23, F24];
    let u = usage as usize;
    Some(match usage {
        4..=29 => *LETTERS.get(u - 4)?,
        30..=39 => *DIGITS.get(u - 30)?,
        40 | 88 => Enter,
        41 => Escape,
        42 => Backspace,
        43 => Tab,
        44 => Space,
        45 | 86 => Minus,
        46 => Equals,
        47 => OpenBracket,
        48 => CloseBracket,
        49 => Backslash,
        51 => Semicolon,
        52 => Quote,
        53 => Backtick,
        54 => Comma,
        55 | 99 => Period,
        56 | 84 => Slash,
        58..=69 => *FKEYS.get(u - 58)?,
        73 => Insert,
        74 => Home,
        75 => PageUp,
        76 => Delete,
        77 => End,
        78 => PageDown,
        79 => ArrowRight,
        80 => ArrowLeft,
        81 => ArrowDown,
        82 => ArrowUp,
        87 => Plus,
        89..=97 => *DIGITS.get(u - 89)?,
        98 => Num0,
        104..=115 => *FKEYS_HIGH.get(u - 104)?,
        _ => return None,
    })
}

/// The logical key a key's unmodified characters name, if they name one (layout-aware: letters,
/// digits and the punctuation FilmCraft binds).
pub fn logical_from_chars(chars: &str) -> Option<Key> {
    let mut it = chars.chars();
    let c = it.next()?;
    if it.next().is_some() {
        return None;
    }
    use Key::*;
    Some(match c.to_ascii_uppercase() {
        'A'..='Z' => {
            let s = c.to_ascii_uppercase().to_string();
            return Key::from_name(&s);
        }
        '0' => Num0,
        '1' => Num1,
        '2' => Num2,
        '3' => Num3,
        '4' => Num4,
        '5' => Num5,
        '6' => Num6,
        '7' => Num7,
        '8' => Num8,
        '9' => Num9,
        '-' => Minus,
        '=' => Equals,
        '+' => Plus,
        '[' => OpenBracket,
        ']' => CloseBracket,
        '\\' => Backslash,
        ';' => Semicolon,
        '\'' => Quote,
        '`' => Backtick,
        ',' => Comma,
        '.' => Period,
        '/' => Slash,
        ':' => Colon,
        '|' => Pipe,
        '?' => Questionmark,
        '!' => Exclamationmark,
        _ => return None,
    })
}

/// The logical key: from the characters when they name one, else the physical key. Named keys
/// (arrows, Return, Tab…) always come from the HID usage: their "characters" are control codes.
pub fn logical(usage: u32, chars_ignoring_modifiers: &str) -> Option<Key> {
    let phys = physical(usage);
    let named = matches!(
        phys,
        Some(
            Key::Enter
                | Key::Escape
                | Key::Backspace
                | Key::Tab
                | Key::Space
                | Key::Insert
                | Key::Home
                | Key::End
                | Key::PageUp
                | Key::PageDown
                | Key::Delete
                | Key::ArrowLeft
                | Key::ArrowRight
                | Key::ArrowUp
                | Key::ArrowDown
        )
    ) || (58..=69).contains(&usage)
        || (104..=115).contains(&usage);
    if named {
        return phys;
    }
    logical_from_chars(chars_ignoring_modifiers).or(phys)
}

/// Text a key press types into a field, if any: printable characters, not with Command or Control
/// (those are shortcuts), and none of UIKit's private-use codes for the arrows and function keys.
pub fn typed_text(chars: &str, m: Modifiers) -> Option<String> {
    if m.command || m.mac_cmd || m.ctrl || chars.is_empty() {
        return None;
    }
    let printable = chars.chars().all(|c| !c.is_control() && !('\u{E000}'..='\u{F8FF}').contains(&c));
    printable.then(|| chars.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hid_usages_map_to_us_positions() {
        assert_eq!(physical(4), Some(Key::A));
        assert_eq!(physical(29), Some(Key::Z));
        assert_eq!(physical(30), Some(Key::Num1));
        assert_eq!(physical(39), Some(Key::Num0));
        assert_eq!(physical(44), Some(Key::Space));
        assert_eq!(physical(80), Some(Key::ArrowLeft));
        assert_eq!(physical(69), Some(Key::F12));
        assert_eq!(physical(0), None);
        assert_eq!(physical(u32::MAX), None);
    }

    #[test]
    fn azerty_letters_are_the_keys_they_are_labelled() {
        // the key at the US "Q" position types "a" on AZERTY: it is Key::A for the shortcuts
        assert_eq!(logical(20, "a"), Some(Key::A));
        assert_eq!(physical(20), Some(Key::Q));
        // the top row types "&" unshifted on AZERTY: still the 1 key
        assert_eq!(logical(30, "&"), Some(Key::Num1));
        // arrows come from the usage, whatever UIKit's private-use characters
        assert_eq!(logical(79, "\u{F703}"), Some(Key::ArrowRight));
        assert_eq!(logical(40, "\r"), Some(Key::Enter));
    }

    #[test]
    fn command_is_the_mac_command() {
        let m = modifiers(MOD_COMMAND | MOD_SHIFT);
        assert!(m.command && m.mac_cmd && m.shift && !m.ctrl && !m.alt);
        assert!(modifiers(MOD_CONTROL).ctrl);
    }

    #[test]
    fn only_printable_unmodified_characters_type() {
        assert_eq!(typed_text("é", Modifiers::NONE), Some("é".into()));
        assert_eq!(typed_text("z", modifiers(MOD_COMMAND)), None);
        assert_eq!(typed_text("\r", Modifiers::NONE), None);
        assert_eq!(typed_text("\u{F700}", Modifiers::NONE), None);
        assert!(is_modifier(227) && is_modifier(57) && !is_modifier(4));
    }
}
