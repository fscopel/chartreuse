//! The settings schema and its defaults.
//!
//! Every field has a default, so a settings file only needs the keys it
//! changes. In the settings file ([`crate::file`]) they look like this, with
//! the defaults shown:
//!
//! ```toml
//! # save_directory = "~/Pictures/Chartreuse"  # absent: the default folder
//! file_name = "Chartreuse {date} at {time}"
//! save_format = "png"            # "png", "jpeg" (or "jpg"), "webp"
//! after_capture = "open_editor"  # "open_editor", "copy", "save_and_copy"
//! launch_at_login = false
//! confirm_close_unsaved = true
//!
//! [hotkeys]
//! display = "Ctrl+Alt+Shift+3"
//! window = "Ctrl+Alt+Shift+5"
//! rectangle = "Ctrl+Alt+Shift+4"
//! ```
//!
//! Hotkeys use [`Hotkey`]'s text form (canonical `Ctrl+Alt+Shift+Super+Key`,
//! aliases such as `Cmd` accepted). `file_name` is a
//! [`FileNamePattern`](crate::pattern).

use std::fmt;
use std::path::{Path, PathBuf};

use chartreuse_core::capture::CaptureMode;
use chartreuse_core::hotkey::{Hotkey, Key, Modifiers};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::format::SaveFormat;
use crate::pattern::FileNamePattern;

/// The folder inside the user's Pictures folder that saves go to by default.
pub const DEFAULT_SAVE_FOLDER: &str = "Chartreuse";

/// Everything the user can configure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Where saves go; `None` means [`default_save_directory`]. Resolve it
    /// with [`Settings::save_directory_path`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_directory: Option<SaveDirectory>,
    /// The name of saved files, without the extension.
    pub file_name: FileNamePattern,
    /// The format files are saved in.
    pub save_format: SaveFormat,
    /// What happens once a capture is taken.
    pub after_capture: AfterCapture,
    /// Whether Chartreuse starts when the user logs in.
    pub launch_at_login: bool,
    /// Whether closing an editor window whose image has not been saved or
    /// copied since it last changed asks first.
    pub confirm_close_unsaved: bool,
    /// The global hotkey of each capture mode.
    pub hotkeys: Hotkeys,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            save_directory: None,
            file_name: FileNamePattern::default(),
            save_format: SaveFormat::default(),
            after_capture: AfterCapture::default(),
            launch_at_login: false,
            confirm_close_unsaved: true,
            hotkeys: Hotkeys::default(),
        }
    }
}

impl Settings {
    /// The directory saves go to: the configured one, or
    /// [`default_save_directory`]. `None` if that needs a home or Pictures
    /// folder the platform does not report.
    #[must_use]
    pub fn save_directory_path(&self) -> Option<PathBuf> {
        match &self.save_directory {
            Some(directory) => directory.resolve(),
            None => default_save_directory(),
        }
    }
}

/// The default save directory: [`DEFAULT_SAVE_FOLDER`] in the user's Pictures
/// folder (`~/Pictures/Chartreuse` on macOS). `None` if the platform reports
/// no Pictures or home folder.
#[must_use]
pub fn default_save_directory() -> Option<PathBuf> {
    dirs::picture_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Pictures")))
        .map(|pictures| pictures.join(DEFAULT_SAVE_FOLDER))
}

/// What happens once a capture is taken.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AfterCapture {
    /// Open the capture in an editor window.
    #[default]
    OpenEditor,
    /// Copy the capture to the clipboard, with no editor.
    Copy,
    /// Save the capture to the save directory and copy it, with no editor.
    SaveAndCopy,
}

impl AfterCapture {
    /// Every behavior, in the order a settings window lists them.
    pub const ALL: [Self; 3] = [Self::OpenEditor, Self::Copy, Self::SaveAndCopy];
}

/// The user-facing description, for example "Open in the editor".
impl fmt::Display for AfterCapture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OpenEditor => "Open in the editor",
            Self::Copy => "Copy to the clipboard",
            Self::SaveAndCopy => "Save and copy to the clipboard",
        })
    }
}

/// A configured save directory, as written: an absolute path, or a path
/// starting with `~` for the user's home folder (`~/Desktop`). A GUI app has
/// no meaningful working directory, so other relative paths are rejected.
///
/// The path is kept as written, so `~` survives a save; [`Self::resolve`]
/// expands it. The settings file stores it as a string, so it must be valid
/// Unicode to be saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PathBuf", into = "PathBuf")]
pub struct SaveDirectory(PathBuf);

impl SaveDirectory {
    /// Validates `path`.
    ///
    /// # Errors
    ///
    /// [`RelativeSaveDirectory`] if it is relative and does not start with `~`.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, RelativeSaveDirectory> {
        let path = path.into();
        if path.is_absolute() || path.starts_with("~") {
            Ok(Self(path))
        } else {
            Err(RelativeSaveDirectory(path))
        }
    }

    /// The path as written.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// The absolute path, with a leading `~` replaced by the home folder.
    /// `None` if it starts with `~` and the platform reports no home folder.
    #[must_use]
    pub fn resolve(&self) -> Option<PathBuf> {
        self.resolve_in(dirs::home_dir().as_deref())
    }

    fn resolve_in(&self, home: Option<&Path>) -> Option<PathBuf> {
        match self.0.strip_prefix("~") {
            Ok(rest) => home.map(|home| home.join(rest)),
            Err(_) => Some(self.0.clone()),
        }
    }
}

impl TryFrom<PathBuf> for SaveDirectory {
    type Error = RelativeSaveDirectory;

    fn try_from(path: PathBuf) -> Result<Self, Self::Error> {
        Self::new(path)
    }
}

impl From<SaveDirectory> for PathBuf {
    fn from(directory: SaveDirectory) -> Self {
        directory.0
    }
}

/// A save directory that is neither absolute nor under `~`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelativeSaveDirectory(pub PathBuf);

impl fmt::Display for RelativeSaveDirectory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the save directory {:?} is not a full path (start it with / or a drive, or with ~ \
             for the home folder)",
            self.0
        )
    }
}

impl std::error::Error for RelativeSaveDirectory {}

impl From<RelativeSaveDirectory> for chartreuse_core::Error {
    fn from(error: RelativeSaveDirectory) -> Self {
        Self::Config(error.to_string())
    }
}

/// The global hotkey of each capture mode. No two modes share a hotkey.
///
/// The defaults are Ctrl+Alt+Shift+3 (display), +5 (window) and +4
/// (rectangle), echoing the system screenshot shortcuts (Shift+Command+3/4/5)
/// without clashing with them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "HotkeysText", into = "HotkeysText")]
pub struct Hotkeys {
    display: Hotkey,
    window: Hotkey,
    rectangle: Hotkey,
}

impl Hotkeys {
    /// The hotkeys for capturing a display, a window, and a rectangle.
    ///
    /// # Errors
    ///
    /// [`DuplicateHotkey`] if two of them are the same.
    pub fn new(
        display: Hotkey,
        window: Hotkey,
        rectangle: Hotkey,
    ) -> Result<Self, DuplicateHotkey> {
        let hotkeys = Self {
            display,
            window,
            rectangle,
        };
        hotkeys.check()?;
        Ok(hotkeys)
    }

    /// The hotkey of `mode`.
    #[must_use]
    pub const fn get(&self, mode: CaptureMode) -> Hotkey {
        match mode {
            CaptureMode::Display => self.display,
            CaptureMode::Window => self.window,
            CaptureMode::Rectangle => self.rectangle,
        }
    }

    /// Makes `hotkey` the hotkey of `mode`.
    ///
    /// # Errors
    ///
    /// [`DuplicateHotkey`] if another mode already uses it; nothing changes.
    pub fn set(&mut self, mode: CaptureMode, hotkey: Hotkey) -> Result<(), DuplicateHotkey> {
        if let Some(other) = CaptureMode::ALL
            .into_iter()
            .find(|&other| other != mode && self.get(other) == hotkey)
        {
            return Err(DuplicateHotkey {
                hotkey,
                modes: [other, mode],
            });
        }
        *self.slot(mode) = hotkey;
        Ok(())
    }

    /// Every mode with its hotkey, in [`CaptureMode::ALL`] order: what to
    /// register with the OS.
    #[must_use]
    pub fn bindings(&self) -> [(CaptureMode, Hotkey); 3] {
        CaptureMode::ALL.map(|mode| (mode, self.get(mode)))
    }

    fn slot(&mut self, mode: CaptureMode) -> &mut Hotkey {
        match mode {
            CaptureMode::Display => &mut self.display,
            CaptureMode::Window => &mut self.window,
            CaptureMode::Rectangle => &mut self.rectangle,
        }
    }

    fn check(&self) -> Result<(), DuplicateHotkey> {
        let bindings = self.bindings();
        for (i, &(first, hotkey)) in bindings.iter().enumerate() {
            if let Some(&(second, _)) = bindings[i + 1..].iter().find(|(_, h)| *h == hotkey) {
                return Err(DuplicateHotkey {
                    hotkey,
                    modes: [first, second],
                });
            }
        }
        Ok(())
    }
}

impl Default for Hotkeys {
    fn default() -> Self {
        let hotkey = |key| Hotkey::new(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT, key);
        Self {
            display: hotkey(Key::Digit3),
            window: hotkey(Key::Digit5),
            rectangle: hotkey(Key::Digit4),
        }
    }
}

/// Two capture modes were given the same hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DuplicateHotkey {
    pub hotkey: Hotkey,
    /// The mode that already had it, then the one it was given to.
    pub modes: [CaptureMode; 2],
}

impl fmt::Display for DuplicateHotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [first, second] = self.modes;
        write!(
            f,
            "{} is the hotkey of both {first} and {second}",
            self.hotkey
        )
    }
}

impl std::error::Error for DuplicateHotkey {}

impl From<DuplicateHotkey> for chartreuse_core::Error {
    fn from(error: DuplicateHotkey) -> Self {
        Self::Config(error.to_string())
    }
}

/// [`Hotkeys`] as stored, before the duplicate check. Missing keys take their
/// default.
#[derive(Serialize, Deserialize)]
#[serde(default)]
struct HotkeysText {
    display: HotkeyText,
    window: HotkeyText,
    rectangle: HotkeyText,
}

impl Default for HotkeysText {
    fn default() -> Self {
        Hotkeys::default().into()
    }
}

impl From<Hotkeys> for HotkeysText {
    fn from(hotkeys: Hotkeys) -> Self {
        Self {
            display: HotkeyText(hotkeys.display),
            window: HotkeyText(hotkeys.window),
            rectangle: HotkeyText(hotkeys.rectangle),
        }
    }
}

impl TryFrom<HotkeysText> for Hotkeys {
    type Error = DuplicateHotkey;

    fn try_from(text: HotkeysText) -> Result<Self, Self::Error> {
        Self::new(text.display.0, text.window.0, text.rectangle.0)
    }
}

/// A [`Hotkey`] stored as its text form.
struct HotkeyText(Hotkey);

impl Serialize for HotkeyText {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for HotkeyText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map(Self).map_err(|error| {
            serde::de::Error::custom(format_args!("invalid hotkey {text:?}: {error}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hotkey(text: &str) -> Hotkey {
        text.parse().unwrap()
    }

    fn from_toml(text: &str) -> Result<Settings, toml_edit::de::Error> {
        toml_edit::de::from_str(text)
    }

    #[test]
    fn defaults_match_the_documented_values() {
        let settings = Settings::default();
        assert_eq!(
            settings.hotkeys.bindings(),
            [
                (CaptureMode::Display, hotkey("Ctrl+Alt+Shift+3")),
                (CaptureMode::Window, hotkey("Ctrl+Alt+Shift+5")),
                (CaptureMode::Rectangle, hotkey("Ctrl+Alt+Shift+4")),
            ]
        );
        assert_eq!(settings.save_directory, None);
        assert_eq!(settings.file_name.as_str(), "Chartreuse {date} at {time}");
        assert_eq!(settings.save_format, SaveFormat::Png);
        assert_eq!(settings.after_capture, AfterCapture::OpenEditor);
        assert!(!settings.launch_at_login);
        assert!(settings.confirm_close_unsaved);
        assert_eq!(settings.save_directory_path(), default_save_directory());
    }

    #[test]
    fn an_empty_document_is_the_defaults() {
        assert_eq!(from_toml("").unwrap(), Settings::default());
    }

    #[test]
    fn missing_keys_keep_their_defaults() {
        let settings =
            from_toml("save_format = \"jpg\"\n[hotkeys]\nwindow = \"Cmd+F5\"\n").unwrap();
        assert_eq!(settings.save_format, SaveFormat::Jpeg);
        assert_eq!(
            settings.hotkeys.get(CaptureMode::Window),
            hotkey("Super+F5")
        );
        assert_eq!(
            settings.hotkeys.get(CaptureMode::Display),
            Hotkeys::default().get(CaptureMode::Display)
        );
        assert_eq!(settings.after_capture, AfterCapture::OpenEditor);
    }

    #[test]
    fn every_field_round_trips_through_toml() {
        let settings = Settings {
            save_directory: Some(SaveDirectory::new("~/Desktop/Shots").unwrap()),
            file_name: FileNamePattern::new("Shot {yyyy}{MM}{dd}").unwrap(),
            save_format: SaveFormat::WebP,
            after_capture: AfterCapture::SaveAndCopy,
            launch_at_login: true,
            confirm_close_unsaved: false,
            hotkeys: Hotkeys::new(hotkey("Super+1"), hotkey("Super+2"), hotkey("Super+3")).unwrap(),
        };
        let text = toml_edit::ser::to_string(&settings).unwrap();
        assert!(text.contains("display = \"Super+1\""), "{text}");
        assert!(text.contains("after_capture = \"save_and_copy\""), "{text}");
        assert_eq!(from_toml(&text).unwrap(), settings);
    }

    #[test]
    fn hotkeys_are_written_in_canonical_form() {
        let settings = from_toml("[hotkeys]\ndisplay = \" shift + cmd + option + 3 \"\n").unwrap();
        let text = toml_edit::ser::to_string(&settings).unwrap();
        assert!(text.contains("display = \"Alt+Shift+Super+3\""), "{text}");
    }

    #[test]
    fn an_invalid_hotkey_is_rejected_with_its_text_and_reason() {
        let error = from_toml("[hotkeys]\nrectangle = \"Ctrl+Hyper+4\"\n").unwrap_err();
        let message = error.message();
        assert!(message.contains("\"Ctrl+Hyper+4\""), "{message}");
        assert!(message.contains("unknown modifier \"Hyper\""), "{message}");
        let error = from_toml("[hotkeys]\nrectangle = \"\"\n").unwrap_err();
        assert!(
            error.message().contains("hotkey is empty"),
            "{}",
            error.message()
        );
    }

    #[test]
    fn duplicate_hotkeys_are_rejected() {
        let error = from_toml("[hotkeys]\nwindow = \"Ctrl+Alt+Shift+3\"\n").unwrap_err();
        let message = error.message();
        assert!(
            message.contains(
                "Ctrl+Alt+Shift+3 is the hotkey of both Capture display and Capture window"
            ),
            "{message}"
        );
    }

    #[test]
    fn setting_a_hotkey_another_mode_uses_changes_nothing() {
        let mut hotkeys = Hotkeys::default();
        let taken = hotkeys.get(CaptureMode::Rectangle);
        assert_eq!(
            hotkeys.set(CaptureMode::Display, taken),
            Err(DuplicateHotkey {
                hotkey: taken,
                modes: [CaptureMode::Rectangle, CaptureMode::Display],
            })
        );
        assert_eq!(hotkeys, Hotkeys::default());
        // Re-setting a mode's own hotkey is not a conflict.
        let own = hotkeys.get(CaptureMode::Display);
        assert_eq!(hotkeys.set(CaptureMode::Display, own), Ok(()));
        hotkeys.set(CaptureMode::Display, hotkey("F13")).unwrap();
        assert_eq!(hotkeys.get(CaptureMode::Display), hotkey("F13"));
    }

    #[test]
    fn unknown_enum_values_are_rejected() {
        let error = from_toml("after_capture = \"upload\"\n").unwrap_err();
        assert!(error.message().contains("upload"), "{}", error.message());
        assert!(from_toml("save_format = \"gif\"\n").is_err());
    }

    #[test]
    fn an_invalid_file_name_pattern_is_rejected() {
        let error = from_toml("file_name = \"shots/{date}\"\n").unwrap_err();
        assert!(
            error.message().contains("cannot contain `/`"),
            "{}",
            error.message()
        );
    }

    #[test]
    fn a_relative_save_directory_is_rejected() {
        assert_eq!(
            SaveDirectory::new("Pictures"),
            Err(RelativeSaveDirectory("Pictures".into()))
        );
        assert!(SaveDirectory::new("").is_err());
        // `~user` is not the home folder.
        assert!(SaveDirectory::new("~bob/Pictures").is_err());
        let error = from_toml("save_directory = \"Pictures\"\n").unwrap_err();
        assert!(
            error.message().contains("not a full path"),
            "{}",
            error.message()
        );
    }

    #[test]
    fn a_leading_tilde_is_the_home_folder() {
        let home = std::env::temp_dir();
        let desktop = SaveDirectory::new("~/Desktop").unwrap();
        assert_eq!(desktop.resolve_in(Some(&home)), Some(home.join("Desktop")));
        assert_eq!(desktop.resolve_in(None), None);
        assert_eq!(
            SaveDirectory::new("~").unwrap().resolve_in(Some(&home)),
            Some(home.clone())
        );
        // An absolute path needs no home folder.
        let absolute = SaveDirectory::new(home.join("Shots")).unwrap();
        assert_eq!(absolute.resolve_in(None), Some(home.join("Shots")));
        let settings = Settings {
            save_directory: Some(absolute),
            ..Settings::default()
        };
        assert_eq!(settings.save_directory_path(), Some(home.join("Shots")));
    }
}
