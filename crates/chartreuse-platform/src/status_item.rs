//! The menu bar / system tray icon and its menu.

use chartreuse_core::capture::CaptureMode;
use chartreuse_core::flavor;
use chartreuse_core::Result;

use crate::event::{EventReceiver, Registration};

/// An action chosen from the status item menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MenuAction {
    Capture(CaptureMode),
    OpenFromClipboard,
    OpenFromFile,
    Settings,
    Quit,
}

impl MenuAction {
    /// The menu item title.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Capture(CaptureMode::Display) => "Capture Display".into(),
            Self::Capture(CaptureMode::Selection) => "Capture Selection".into(),
            Self::OpenFromClipboard => "Open from Clipboard".into(),
            Self::OpenFromFile => "Open File…".into(),
            Self::Settings => "Settings…".into(),
            Self::Quit => format!("Quit {}", flavor::DISPLAY_NAME),
        }
    }
}

/// One row of the status item menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuEntry {
    Action(MenuAction),
    Separator,
}

/// The status item menu, top to bottom. Every backend shows exactly this.
pub const MENU: &[MenuEntry] = &[
    MenuEntry::Action(MenuAction::Capture(CaptureMode::Display)),
    MenuEntry::Action(MenuAction::Capture(CaptureMode::Selection)),
    MenuEntry::Separator,
    MenuEntry::Action(MenuAction::OpenFromClipboard),
    MenuEntry::Action(MenuAction::OpenFromFile),
    MenuEntry::Separator,
    MenuEntry::Action(MenuAction::Settings),
    MenuEntry::Action(MenuAction::Quit),
];

/// The result of [`StatusItem::install`].
#[derive(Debug)]
pub struct StatusItemHandle {
    /// Menu choices, as the user makes them.
    pub actions: EventReceiver<MenuAction>,
    /// Keeps the icon installed; drop it (on the main thread) to remove it.
    pub registration: Registration,
}

/// The status item: an icon in the menu bar (macOS) or tray (Windows, Linux) with
/// the [`MENU`].
pub trait StatusItem {
    /// Shows the icon and menu. On macOS this also switches the app to the
    /// accessory activation policy (no Dock icon).
    ///
    /// **Main thread only** (`NSStatusItem`). Call it from iced `update`, not
    /// `boot`: `boot` runs before the AppKit run loop starts, and a status item
    /// created then misbehaves (for example alongside full-screen apps). At
    /// startup, have `boot` return `Task::done` with an install message and
    /// install when `update` handles it (see the crate's Threading docs).
    fn install(&self) -> Result<StatusItemHandle>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_offers_every_action_once_without_stray_separators() {
        let actions: Vec<MenuAction> = MENU
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Action(action) => Some(*action),
                MenuEntry::Separator => None,
            })
            .collect();
        let mut expected: Vec<MenuAction> = CaptureMode::ALL.map(MenuAction::Capture).to_vec();
        expected.extend([
            MenuAction::OpenFromClipboard,
            MenuAction::OpenFromFile,
            MenuAction::Settings,
            MenuAction::Quit,
        ]);
        assert_eq!(actions, expected);
        assert_ne!(MENU.first(), Some(&MenuEntry::Separator));
        assert_ne!(MENU.last(), Some(&MenuEntry::Separator));
        assert!(!MENU
            .windows(2)
            .any(|pair| pair == [MenuEntry::Separator; 2]));
    }
}
