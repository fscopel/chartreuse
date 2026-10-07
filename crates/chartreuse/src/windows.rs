//! The window registry: which module owns each open window. Also the icon the
//! app's windows show.

use std::collections::HashMap;

use chartreuse_core::flavor::{self, Flavor};
use iced::{window, Task};

/// What a window is for, and therefore which module draws it and hears when it
/// closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowKind {
    /// A selection overlay on one display (`overlay`).
    Overlay,
    /// An annotation editor (`editor`).
    Editor,
    /// The settings window (`settings`).
    Settings,
    /// A user notice from `report_error` (`alert`).
    Alert,
    /// Screen Recording permission guidance (`permission`).
    Permission,
}

impl WindowKind {
    /// The window title.
    #[must_use]
    pub fn title(self) -> String {
        let name = flavor::DISPLAY_NAME;
        match self {
            Self::Overlay | Self::Alert => name.to_owned(),
            Self::Editor => format!("{name} Editor"),
            Self::Settings => format!("{name} Settings"),
            Self::Permission => format!("{name} Needs Screen Recording"),
        }
    }
}

/// Maps every open window to its [`WindowKind`].
#[derive(Debug, Default)]
pub struct WindowRegistry {
    windows: HashMap<window::Id, WindowKind>,
}

impl WindowRegistry {
    /// Opens a window of `kind` and records it. Returns the new id and the task
    /// that opens it (resolving to the same id once the window exists).
    pub fn open(
        &mut self,
        kind: WindowKind,
        settings: window::Settings,
    ) -> (window::Id, Task<window::Id>) {
        let (id, task) = window::open(settings);
        self.windows.insert(id, kind);
        (id, task)
    }

    /// The kind of an open window.
    #[must_use]
    pub fn kind(&self, id: window::Id) -> Option<WindowKind> {
        self.windows.get(&id).copied()
    }

    /// Forgets a closed window, returning what it was.
    pub fn remove(&mut self, id: window::Id) -> Option<WindowKind> {
        self.windows.remove(&id)
    }

    /// The open windows of one kind, in no particular order.
    pub fn of_kind(&self, kind: WindowKind) -> impl Iterator<Item = window::Id> + '_ {
        self.windows
            .iter()
            .filter(move |(_, k)| **k == kind)
            .map(|(id, _)| *id)
    }

    /// The number of open windows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.windows.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }
}

/// The app icon in the build flavor's accent, shown at the left of the title
/// bar on Windows and by X11 window managers (macOS windows have none, and
/// Wayland compositors look up the icon themselves). The 32-pixel small icon:
/// what the executable's icon holds at that size, sharp at 200% scale and
/// halved at 100%.
#[must_use]
pub fn icon() -> Option<window::Icon> {
    let png: &[u8] = match Flavor::CURRENT {
        Flavor::Release => include_bytes!("../../../assets/icon/generated/tray-release-32.png"),
        Flavor::Development => {
            include_bytes!("../../../assets/icon/generated/tray-development-32.png")
        }
    };
    window::icon::from_file_data(png, None)
        .inspect_err(|error| tracing::warn!(%error, "could not load the window icon"))
        .ok()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn icon_decodes_to_the_32_pixel_app_icon() {
        let (_, size) = icon().expect("the icon decodes").into_raw();
        assert_eq!((size.width, size.height), (32, 32));
    }

    #[test]
    fn icon_is_in_the_build_flavors_accent() {
        let (rgba, size) = icon().expect("the icon decodes").into_raw();
        // The middle of the left edge: the icon's body, clear of the mark and
        // of the sheen at the top.
        let at = 4 * (size.width * (size.height / 2) + 1) as usize;
        let body = &rgba[at..at + 3];
        let accent = flavor::ACCENT;
        // Rendering rounds the accent by a step.
        let close = body
            .iter()
            .zip([accent.r, accent.g, accent.b])
            .all(|(channel, accent)| channel.abs_diff(accent) <= 1);
        assert!(close, "{body:?} is not {accent}");
    }

    #[test]
    fn windows_are_tracked_by_kind_until_removed() {
        let mut registry = WindowRegistry::default();
        let (editor, _) = registry.open(WindowKind::Editor, window::Settings::default());
        let (alert_a, _) = registry.open(WindowKind::Alert, window::Settings::default());
        let (alert_b, _) = registry.open(WindowKind::Alert, window::Settings::default());
        assert_eq!(registry.kind(editor), Some(WindowKind::Editor));

        let alerts: HashSet<_> = registry.of_kind(WindowKind::Alert).collect();
        assert_eq!(alerts, HashSet::from([alert_a, alert_b]));

        assert_eq!(registry.remove(alert_a), Some(WindowKind::Alert));
        assert_eq!(registry.remove(alert_a), None, "a window closes only once");
        assert_eq!(registry.kind(alert_a), None);
        assert_eq!(registry.len(), 2);
    }
}
