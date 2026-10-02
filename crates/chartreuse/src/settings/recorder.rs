//! The hotkey recorder's key handling: what a key press means while a hotkey
//! is being recorded, and how the settings window shows hotkeys.
//!
//! Keys are taken by their physical position ([`Physical`]), as global
//! hotkeys are registered: Option+3 on a Mac types `£`, but it is still the
//! 3 key.

use chartreuse_core::hotkey::{Hotkey, Key, Modifiers};
use iced::keyboard;
use iced::keyboard::key::{Code, Physical};

/// What a key press means to the recorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Recorded {
    /// The new hotkey.
    Hotkey(Hotkey),
    /// Escape: stop recording and keep the hotkey as it was.
    Cancel,
    /// A modifier key on its own: keep waiting for the rest of the
    /// combination.
    Modifier,
    /// A key without Ctrl, Alt or Super (Shift alone is not enough), which as
    /// a global hotkey would stop typing it in every other app. PrintScreen
    /// types nothing, so it needs no modifier.
    NeedsModifier,
    /// A key a global hotkey cannot use, such as a keypad or media key.
    Unsupported,
}

/// The line under a hotkey that was pressed without a modifier.
pub(super) const NEEDS_MODIFIER: &str = if cfg!(target_os = "macos") {
    "Include Control, Option or Command, so the key keeps working in other apps"
} else if cfg!(windows) {
    "Include Ctrl, Alt or the Windows key, so the key keeps working in other apps"
} else {
    "Include Ctrl, Alt or Super, so the key keeps working in other apps"
};

/// The line under a hotkey that was pressed with a key hotkeys cannot use.
pub(super) const UNSUPPORTED: &str = "That key cannot be part of a hotkey";

/// What pressing `key` with `modifiers` held means to the recorder.
pub(super) fn recorded(key: Physical, modifiers: keyboard::Modifiers) -> Recorded {
    let Physical::Code(code) = key else {
        return Recorded::Unsupported;
    };
    if code == Code::Escape {
        return Recorded::Cancel;
    }
    if MODIFIER_KEYS.contains(&code) {
        return Recorded::Modifier;
    }
    let Some(&(_, key)) = KEYS.iter().find(|(known, _)| *known == code) else {
        return Recorded::Unsupported;
    };
    let modifiers = modifiers_of(modifiers);
    if key == Key::PrintScreen
        || [Modifiers::CONTROL, Modifiers::ALT, Modifiers::SUPER]
            .into_iter()
            .any(|modifier| modifiers.contains(modifier))
    {
        Recorded::Hotkey(Hotkey::new(modifiers, key))
    } else {
        Recorded::NeedsModifier
    }
}

/// How the settings window shows `hotkey`: with the modifier symbols macOS
/// menus use on macOS (`⌃⌥⇧⌘3`), as its text form elsewhere
/// (`Ctrl+Alt+Shift+Super+3`).
pub(super) fn label(hotkey: Hotkey) -> String {
    if cfg!(target_os = "macos") {
        symbols(hotkey)
    } else {
        hotkey.to_string()
    }
}

/// `hotkey` with macOS's modifier symbols, in macOS's order.
fn symbols(hotkey: Hotkey) -> String {
    let mut label: String = [
        (Modifiers::CONTROL, '⌃'),
        (Modifiers::ALT, '⌥'),
        (Modifiers::SHIFT, '⇧'),
        (Modifiers::SUPER, '⌘'),
    ]
    .into_iter()
    .filter(|&(modifier, _)| hotkey.modifiers.contains(modifier))
    .map(|(_, symbol)| symbol)
    .collect();
    label.push_str(hotkey.key.name());
    label
}

fn modifiers_of(held: keyboard::Modifiers) -> Modifiers {
    [
        (held.control(), Modifiers::CONTROL),
        (held.alt(), Modifiers::ALT),
        (held.shift(), Modifiers::SHIFT),
        (held.logo(), Modifiers::SUPER),
    ]
    .into_iter()
    .filter(|&(is_held, _)| is_held)
    .fold(Modifiers::NONE, |all, (_, modifier)| all | modifier)
}

/// Keys that only modify others.
const MODIFIER_KEYS: &[Code] = &[
    Code::ShiftLeft,
    Code::ShiftRight,
    Code::ControlLeft,
    Code::ControlRight,
    Code::AltLeft,
    Code::AltRight,
    Code::SuperLeft,
    Code::SuperRight,
    Code::Meta,
    Code::Hyper,
    Code::Fn,
    Code::FnLock,
    Code::CapsLock,
];

/// The hotkey key at each physical position. Escape is missing: it cancels
/// recording, so a hotkey with it can only be set in the settings file.
const KEYS: &[(Code, Key)] = &[
    (Code::KeyA, Key::A),
    (Code::KeyB, Key::B),
    (Code::KeyC, Key::C),
    (Code::KeyD, Key::D),
    (Code::KeyE, Key::E),
    (Code::KeyF, Key::F),
    (Code::KeyG, Key::G),
    (Code::KeyH, Key::H),
    (Code::KeyI, Key::I),
    (Code::KeyJ, Key::J),
    (Code::KeyK, Key::K),
    (Code::KeyL, Key::L),
    (Code::KeyM, Key::M),
    (Code::KeyN, Key::N),
    (Code::KeyO, Key::O),
    (Code::KeyP, Key::P),
    (Code::KeyQ, Key::Q),
    (Code::KeyR, Key::R),
    (Code::KeyS, Key::S),
    (Code::KeyT, Key::T),
    (Code::KeyU, Key::U),
    (Code::KeyV, Key::V),
    (Code::KeyW, Key::W),
    (Code::KeyX, Key::X),
    (Code::KeyY, Key::Y),
    (Code::KeyZ, Key::Z),
    (Code::Digit0, Key::Digit0),
    (Code::Digit1, Key::Digit1),
    (Code::Digit2, Key::Digit2),
    (Code::Digit3, Key::Digit3),
    (Code::Digit4, Key::Digit4),
    (Code::Digit5, Key::Digit5),
    (Code::Digit6, Key::Digit6),
    (Code::Digit7, Key::Digit7),
    (Code::Digit8, Key::Digit8),
    (Code::Digit9, Key::Digit9),
    (Code::F1, Key::F1),
    (Code::F2, Key::F2),
    (Code::F3, Key::F3),
    (Code::F4, Key::F4),
    (Code::F5, Key::F5),
    (Code::F6, Key::F6),
    (Code::F7, Key::F7),
    (Code::F8, Key::F8),
    (Code::F9, Key::F9),
    (Code::F10, Key::F10),
    (Code::F11, Key::F11),
    (Code::F12, Key::F12),
    (Code::F13, Key::F13),
    (Code::F14, Key::F14),
    (Code::F15, Key::F15),
    (Code::F16, Key::F16),
    (Code::F17, Key::F17),
    (Code::F18, Key::F18),
    (Code::F19, Key::F19),
    (Code::F20, Key::F20),
    (Code::Space, Key::Space),
    (Code::Tab, Key::Tab),
    (Code::Enter, Key::Return),
    (Code::Backspace, Key::Backspace),
    (Code::Delete, Key::Delete),
    (Code::Insert, Key::Insert),
    (Code::Home, Key::Home),
    (Code::End, Key::End),
    (Code::PageUp, Key::PageUp),
    (Code::PageDown, Key::PageDown),
    (Code::ArrowLeft, Key::Left),
    (Code::ArrowRight, Key::Right),
    (Code::ArrowUp, Key::Up),
    (Code::ArrowDown, Key::Down),
    (Code::Minus, Key::Minus),
    (Code::Equal, Key::Equal),
    (Code::BracketLeft, Key::LeftBracket),
    (Code::BracketRight, Key::RightBracket),
    (Code::Backslash, Key::Backslash),
    (Code::Semicolon, Key::Semicolon),
    (Code::Quote, Key::Quote),
    (Code::Comma, Key::Comma),
    (Code::Period, Key::Period),
    (Code::Slash, Key::Slash),
    (Code::Backquote, Key::Grave),
    (Code::PrintScreen, Key::PrintScreen),
];

#[cfg(test)]
mod tests {
    use iced::keyboard::key::NativeCode;

    use super::*;

    fn press(code: Code, held: keyboard::Modifiers) -> Recorded {
        recorded(Physical::Code(code), held)
    }

    fn hotkey(text: &str) -> Recorded {
        Recorded::Hotkey(text.parse().unwrap())
    }

    const CMD_SHIFT: keyboard::Modifiers =
        keyboard::Modifiers::LOGO.union(keyboard::Modifiers::SHIFT);

    #[test]
    fn a_key_with_ctrl_alt_or_super_becomes_the_hotkey() {
        assert_eq!(press(Code::KeyK, CMD_SHIFT), hotkey("Shift+Super+K"));
        assert_eq!(
            press(
                Code::F5,
                keyboard::Modifiers::CTRL | keyboard::Modifiers::ALT
            ),
            hotkey("Ctrl+Alt+F5")
        );
        assert_eq!(
            press(Code::Digit3, keyboard::Modifiers::ALT),
            hotkey("Alt+3"),
            "by position, whatever Option+3 types"
        );
        assert_eq!(
            press(Code::BracketLeft, keyboard::Modifiers::CTRL),
            hotkey("Ctrl+[")
        );
    }

    #[test]
    fn a_key_without_ctrl_alt_or_super_is_not_a_hotkey() {
        for held in [keyboard::Modifiers::empty(), keyboard::Modifiers::SHIFT] {
            assert_eq!(press(Code::KeyK, held), Recorded::NeedsModifier, "{held:?}");
            assert_eq!(press(Code::F5, held), Recorded::NeedsModifier, "{held:?}");
        }
    }

    #[test]
    fn print_screen_needs_no_modifier() {
        assert_eq!(
            press(Code::PrintScreen, keyboard::Modifiers::empty()),
            hotkey("PrintScreen")
        );
        assert_eq!(
            press(Code::PrintScreen, keyboard::Modifiers::SHIFT),
            hotkey("Shift+PrintScreen")
        );
    }

    #[test]
    fn escape_cancels_and_modifiers_alone_keep_waiting() {
        assert_eq!(
            press(Code::Escape, keyboard::Modifiers::empty()),
            Recorded::Cancel
        );
        assert_eq!(press(Code::Escape, CMD_SHIFT), Recorded::Cancel);
        assert_eq!(
            press(Code::ShiftLeft, keyboard::Modifiers::SHIFT),
            Recorded::Modifier
        );
        assert_eq!(press(Code::SuperRight, CMD_SHIFT), Recorded::Modifier);
    }

    #[test]
    fn keys_hotkeys_cannot_use_are_unsupported() {
        assert_eq!(press(Code::Numpad1, CMD_SHIFT), Recorded::Unsupported);
        assert_eq!(
            press(Code::MediaPlayPause, CMD_SHIFT),
            Recorded::Unsupported
        );
        assert_eq!(
            recorded(Physical::Unidentified(NativeCode::Unidentified), CMD_SHIFT),
            Recorded::Unsupported
        );
    }

    #[test]
    fn every_hotkey_key_can_be_recorded() {
        for key in Key::ALL.iter().filter(|&&key| key != Key::Escape) {
            assert!(
                KEYS.iter().any(|(_, known)| known == key),
                "no physical key records {key:?}"
            );
        }
    }

    #[test]
    fn macos_symbols_come_in_the_order_macos_menus_use() {
        let hotkey = "Ctrl+Alt+Shift+Super+3".parse().unwrap();
        assert_eq!(symbols(hotkey), "⌃⌥⇧⌘3");
        assert_eq!(symbols("Super+Shift+F5".parse().unwrap()), "⇧⌘F5");
    }
}
