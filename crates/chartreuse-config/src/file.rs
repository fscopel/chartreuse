//! The settings file: `settings.toml` in a per-flavor configuration directory.
//!
//! [`settings_path`] is [`FILE_NAME`] in [`config_dir`], the platform's
//! configuration folder plus the flavor's bundle identifier, so a development
//! build never touches a user's real settings:
//!
//! | Platform | Development build                                                     |
//! |----------|-----------------------------------------------------------------------|
//! | macOS    | `~/Library/Application Support/io.jennings.chartreuse.dev/settings.toml` |
//! | Windows  | `%APPDATA%\io.jennings.chartreuse.dev\settings.toml`                  |
//! | Linux    | `$XDG_CONFIG_HOME/io.jennings.chartreuse.dev/settings.toml` (`~/.config`) |
//!
//! The file is meant to be edited by hand as well as by the settings window
//! (the keys are described in [`crate::settings`]):
//!
//! - [`load`]: a missing file means the defaults, and so does a missing key.
//!   A file that is not valid TOML, or holds an invalid value, is an
//!   [`Error::Config`] naming the file, the line and column, and the key; the
//!   file is left alone for the user to fix.
//! - Keys Chartreuse does not know (a newer version's settings, or a typo)
//!   are ignored when loading, with a logged warning, and kept when saving.
//! - [`save`] writes into the file as it is on disk, so comments, formatting
//!   and unknown keys survive; a setting back at its default that may be left
//!   out (`save_directory`) is removed. It refuses to replace a file that is
//!   not valid TOML. The write is atomic: a temporary file in the same
//!   directory is renamed over the old one, and the directory is created if
//!   needed. A symlinked settings file stays a symlink.

use std::fs;
use std::io::{self, Write as _};
use std::ops::Range;
use std::path::{Path, PathBuf};

use chartreuse_core::flavor::Flavor;
use chartreuse_core::{Error, Result};
use serde::Deserialize as _;
use toml_edit::{DocumentMut, Item, TableLike, Value};

use crate::settings::{SaveDirectory, Settings};

/// The settings file's name inside [`config_dir`].
pub const FILE_NAME: &str = "settings.toml";

/// The comment a new settings file starts with.
const HEADER: &str = "\
# Chartreuse settings. You can edit this file by hand; Chartreuse keeps your
# comments, and settings it does not know, when it saves.
#
# save_directory: a full path, or one starting with ~ (default: the
#   Chartreuse folder in Pictures)
# file_name: {date} {time} {yyyy} {MM} {dd} {HH} {mm} {ss} are replaced by
#   the capture's date and time
# save_format: \"png\", \"jpeg\" or \"webp\"
# after_capture: \"open_editor\", \"copy\" or \"save_and_copy\"
# confirm_close_unsaved: ask before closing an editor whose image has not
#   been saved or copied
# hotkeys: modifiers Ctrl, Alt, Shift, Super (Cmd on macOS) plus a key

";

/// The configuration directory of `flavor`: the platform's configuration
/// folder plus the flavor's bundle identifier. `None` if the platform
/// reports no configuration folder.
#[must_use]
pub fn config_dir(flavor: Flavor) -> Option<PathBuf> {
    dirs::config_dir().map(|config| config.join(flavor.bundle_id()))
}

/// The settings file of this build's flavor.
///
/// # Errors
///
/// [`Error::Config`] if the platform reports no configuration folder.
pub fn settings_path() -> Result<PathBuf> {
    config_dir(Flavor::CURRENT)
        .map(|directory| directory.join(FILE_NAME))
        .ok_or_else(|| Error::Config("there is no configuration folder to keep them in".into()))
}

/// Reads the settings file at `path`; a missing file gives the defaults.
/// Unknown keys are logged and ignored.
///
/// # Errors
///
/// [`Error::Config`] if the file is not UTF-8, not valid TOML, or holds an
/// invalid setting: the message names the file, the line and column, and the
/// key. [`Error::Io`] if it cannot be read.
pub fn load(path: &Path) -> Result<Settings> {
    let Some(text) = read(path)? else {
        return Ok(Settings::default());
    };
    let parsed = parse(&text).map_err(|problem| problem.into_error(path, &text))?;
    for key in &parsed.unknown_keys {
        tracing::warn!(file = %path.display(), key, "ignoring an unknown setting");
    }
    Ok(parsed.settings)
}

/// Writes `settings` to the file at `path`, keeping its comments, formatting
/// and unknown keys (a new file gets an explanatory header). Atomic; creates
/// the directory if needed. Blocking.
///
/// # Errors
///
/// [`Error::Config`] if the existing file is not valid TOML (it is left
/// alone), or a setting cannot be stored (a save directory that is not
/// Unicode). [`Error::Io`] if the file or directory cannot be written.
pub fn save(path: &Path, settings: &Settings) -> Result<()> {
    // Replace the file a symlink points to, not the symlink.
    let is_symlink = fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink());
    let path = if is_symlink {
        follow(path)?
    } else {
        path.to_owned()
    };
    let existing = read(&path)?;
    let mut document = match &existing {
        Some(text) => text.parse::<DocumentMut>().map_err(|error| {
            Problem {
                message: format!(
                    "{}; fix it or delete the file before changing settings",
                    error.message()
                ),
                span: error.span(),
                key: None,
            }
            .into_error(&path, text)
        })?,
        None => DocumentMut::new(),
    };
    let values = to_document(settings)?;
    merge(
        document.as_table_mut(),
        values.as_table(),
        known_keys().as_table(),
    );
    let mut text = if existing.is_some() {
        String::new()
    } else {
        HEADER.to_owned()
    };
    text.push_str(&document.to_string());
    write_atomically(&path, text.as_bytes())
}

/// The file the symlink `link` points to. A dangling link is followed one
/// level, so saving creates the file it names.
fn follow(link: &Path) -> Result<PathBuf> {
    let following = |error| Error::io(format!("following {}", link.display()), error);
    match fs::canonicalize(link) {
        Ok(target) => Ok(target),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let target = fs::read_link(link).map_err(following)?;
            // A relative target is relative to the link's directory.
            Ok(match link.parent() {
                Some(parent) => parent.join(target),
                None => target,
            })
        }
        Err(error) => Err(following(error)),
    }
}

/// `settings` as a document, with sections such as `hotkeys` as `[tables]`
/// (the serializer writes them inline).
fn to_document(settings: &Settings) -> Result<DocumentMut> {
    let mut document = toml_edit::ser::to_document(settings)
        .map_err(|error| Error::Config(format!("the settings cannot be saved: {error}")))?;
    for (_, item) in document.as_table_mut().iter_mut() {
        if item.is_inline_table() {
            *item = match std::mem::take(item).into_table() {
                Ok(table) => Item::Table(table),
                Err(item) => item,
            };
        }
    }
    Ok(document)
}

/// The file's text, or `None` if it does not exist.
fn read(path: &Path) -> Result<Option<String>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::io(format!("reading {}", path.display()), error)),
    };
    String::from_utf8(bytes).map(Some).map_err(|error| {
        Error::Config(format!(
            "{} is not a text file (invalid UTF-8 at byte {})",
            path.display(),
            error.utf8_error().valid_up_to()
        ))
    })
}

/// A settings file's contents.
#[derive(Debug)]
struct Parsed {
    settings: Settings,
    /// Dotted paths of the keys no setting reads, such as `hotkeys.scren`.
    unknown_keys: Vec<String>,
}

/// What is wrong with a settings file, before it is tied to the file.
#[derive(Debug)]
struct Problem {
    message: String,
    /// Byte offsets into the text.
    span: Option<Range<usize>>,
    /// The dotted key the problem is in.
    key: Option<String>,
}

impl Problem {
    fn into_error(self, path: &Path, text: &str) -> Error {
        let mut location = Vec::new();
        if let Some(span) = self.span {
            let (line, column) = line_column(text, span.start);
            location.push(format!("line {line}, column {column}"));
        }
        if let Some(key) = self.key {
            location.push(format!("key {key}"));
        }
        let location = if location.is_empty() {
            String::new()
        } else {
            format!(" ({})", location.join(", "))
        };
        Error::Config(format!("{}{location}: {}", path.display(), self.message))
    }
}

fn parse(text: &str) -> Result<Parsed, Problem> {
    let document = toml_edit::Document::parse(text).map_err(|error| Problem {
        message: error.message().to_owned(),
        span: error.span(),
        key: None,
    })?;
    let deserializer = toml_edit::de::Deserializer::from(document.clone());
    let settings = Settings::deserialize(deserializer).map_err(|error| {
        let span = error.span();
        let key = span
            .as_ref()
            .and_then(|span| key_at(document.as_table(), span.start))
            .map(|path| path.join("."));
        Problem {
            message: error.message().to_owned(),
            span,
            key,
        }
    })?;
    let mut unknown_keys = Vec::new();
    find_unknown(
        document.as_table(),
        known_keys().as_table(),
        "",
        &mut unknown_keys,
    );
    Ok(Parsed {
        settings,
        unknown_keys,
    })
}

/// The 1-based line and column (in characters) of byte `offset` in `text`.
fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let before = text.get(..offset).unwrap_or(text);
    let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
    let line = before.matches('\n').count() + 1;
    (line, before[line_start..].chars().count() + 1)
}

/// The path of the innermost key whose name or value covers byte `offset`.
fn key_at(table: &dyn TableLike, offset: usize) -> Option<Vec<String>> {
    let covers = |span: Option<Range<usize>>| span.is_some_and(|span| span.contains(&offset));
    for (name, item) in table.iter() {
        if let Some(inner) = item.as_table_like()
            && let Some(mut path) = key_at(inner, offset)
        {
            path.insert(0, name.to_owned());
            return Some(path);
        }
        if covers(table.key(name).and_then(toml_edit::Key::span)) || covers(item.span()) {
            return Some(vec![name.to_owned()]);
        }
    }
    None
}

/// Every key a settings file can hold, as a document.
fn known_keys() -> DocumentMut {
    let every_key = Settings {
        save_directory: Some(SaveDirectory::new("~").expect("`~` is a valid save directory")),
        ..Settings::default()
    };
    toml_edit::ser::to_document(&every_key).expect("the settings serialize")
}

/// Appends to `unknown` the dotted path of each key in `table` that `known`
/// lacks, without descending into unknown tables.
fn find_unknown(
    table: &dyn TableLike,
    known: &dyn TableLike,
    prefix: &str,
    unknown: &mut Vec<String>,
) {
    for (name, item) in table.iter() {
        let path = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}.{name}")
        };
        match known.get(name) {
            None => unknown.push(path),
            Some(known_item) => {
                if let (Some(inner), Some(known_inner)) =
                    (item.as_table_like(), known_item.as_table_like())
                {
                    find_unknown(inner, known_inner, &path, unknown);
                }
            }
        }
    }
}

/// Writes `values` into `target`, keeping `target`'s formatting and its keys
/// that `known` lacks. Known keys that `values` leaves out are removed.
fn merge(target: &mut dyn TableLike, values: &dyn TableLike, known: &dyn TableLike) {
    for (name, value) in values.iter() {
        match target.get_mut(name) {
            Some(old) => update(old, value, known.get(name)),
            None => {
                target.insert(name, value.clone());
            }
        }
    }
    let left_out: Vec<String> = target
        .iter()
        .filter(|(name, _)| known.contains_key(name) && !values.contains_key(name))
        .map(|(name, _)| name.to_owned())
        .collect();
    for name in left_out {
        target.remove(&name);
    }
}

/// Replaces `old` with `new`, merging tables and keeping a value's comments.
fn update(old: &mut Item, new: &Item, known: Option<&Item>) {
    if let (Some(known), Some(new_table)) =
        (known.and_then(Item::as_table_like), new.as_table_like())
        && let Some(old_table) = old.as_table_like_mut()
    {
        merge(old_table, new_table, known);
        return;
    }
    match (old.as_value_mut(), new.as_value()) {
        (Some(old_value), Some(new_value)) => {
            if !same_value(old_value, new_value) {
                let decor = old_value.decor().clone();
                *old_value = new_value.clone();
                *old_value.decor_mut() = decor;
            }
        }
        _ => *old = new.clone(),
    }
}

/// Whether two values are equal, whatever their formatting. Only the kinds
/// settings use are compared; others count as different.
fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(a), Value::String(b)) => a.value() == b.value(),
        (Value::Boolean(a), Value::Boolean(b)) => a.value() == b.value(),
        _ => false,
    }
}

/// Replaces `path` with `contents` by renaming a temporary file over it.
fn write_atomically(path: &Path, contents: &[u8]) -> Result<()> {
    let directory = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    fs::create_dir_all(directory)
        .map_err(|error| Error::io(format!("creating {}", directory.display()), error))?;
    let writing = |error| Error::io(format!("writing {}", path.display()), error);
    let mut file = tempfile::Builder::new()
        .prefix(".settings")
        .suffix(".tmp")
        .tempfile_in(directory)
        .map_err(writing)?;
    file.write_all(contents)
        .and_then(|()| file.as_file().sync_all())
        .map_err(writing)?;
    // The temporary file is deleted if this fails.
    file.persist(path).map_err(|error| writing(error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use chartreuse_core::capture::CaptureMode;
    use chartreuse_core::hotkey::Hotkey;

    use super::*;
    use crate::format::SaveFormat;
    use crate::pattern::FileNamePattern;
    use crate::settings::{AfterCapture, Hotkeys};

    fn hotkey(text: &str) -> Hotkey {
        text.parse().unwrap()
    }

    fn custom() -> Settings {
        Settings {
            save_directory: Some(SaveDirectory::new("~/Desktop").unwrap()),
            file_name: FileNamePattern::new("Shot {date} {HH}{mm}").unwrap(),
            save_format: SaveFormat::Jpeg,
            after_capture: AfterCapture::Copy,
            launch_at_login: true,
            confirm_close_unsaved: false,
            hotkeys: Hotkeys::new(hotkey("Super+F1"), hotkey("Super+F2")).unwrap(),
        }
    }

    /// A temporary directory and the settings path inside it.
    fn scratch() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        (directory, path)
    }

    fn config_error(result: Result<Settings>) -> String {
        match result {
            Err(Error::Config(message)) => message,
            other => panic!("expected a config error, got {other:?}"),
        }
    }

    #[test]
    fn each_flavor_has_its_own_directory_named_after_its_bundle_id() {
        let (Some(dev), Some(release)) =
            (config_dir(Flavor::Development), config_dir(Flavor::Release))
        else {
            return; // No configuration folder on this machine.
        };
        assert!(dev.ends_with("io.jennings.chartreuse.dev"), "{dev:?}");
        assert!(release.ends_with("io.jennings.chartreuse"), "{release:?}");
        assert_eq!(dev.parent(), release.parent());
    }

    #[test]
    fn a_missing_file_is_the_defaults_and_is_not_created() {
        let (_directory, path) = scratch();
        assert_eq!(load(&path).unwrap(), Settings::default());
        assert!(!path.exists());
    }

    #[test]
    fn defaults_round_trip_through_a_new_file() {
        let (directory, _) = scratch();
        let path = directory.path().join("new").join("folder").join(FILE_NAME);
        save(&path, &Settings::default()).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# Chartreuse settings."), "{text}");
        assert!(
            text.contains("[hotkeys]\ndisplay = \"Ctrl+Alt+Shift+3\""),
            "{text}"
        );
        assert!(!text.contains("save_directory ="), "{text}");
        assert_eq!(load(&path).unwrap(), Settings::default());
        // Only the settings file is left behind.
        let entries: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn every_setting_round_trips_through_the_file() {
        let (_directory, path) = scratch();
        save(&path, &Settings::default()).unwrap();
        save(&path, &custom()).unwrap();
        assert_eq!(load(&path).unwrap(), custom());
        save(&path, &Settings::default()).unwrap();
        assert_eq!(load(&path).unwrap(), Settings::default());
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("save_directory ="),
            "back to the default: {text}"
        );
    }

    #[test]
    fn unknown_keys_are_ignored_when_loading() {
        let parsed = parse(
            "future = 1\nlaunch_at_login = true\n[hotkeys]\nscren = \"F1\"\n[plugins.x]\ny = 2\n",
        )
        .unwrap();
        assert!(parsed.settings.launch_at_login);
        assert_eq!(parsed.settings.hotkeys, Hotkeys::default());
        assert_eq!(parsed.unknown_keys, ["future", "hotkeys.scren", "plugins"]);
        assert!(parse(&known_keys().to_string())
            .unwrap()
            .unknown_keys
            .is_empty());
    }

    #[test]
    fn saving_keeps_comments_formatting_and_unknown_keys() {
        let (_directory, path) = scratch();
        let original = "\
# My settings
future_setting = \"keep me\"
after_capture = 'copy'  # straight to the clipboard

# Hotkeys I like
[hotkeys]
display = \"Cmd+Shift+3\"  # muscle memory
extra = [1, 2]

[plugins]
enabled = true
";
        fs::write(&path, original).unwrap();
        let mut settings = load(&path).unwrap();
        assert_eq!(settings.after_capture, AfterCapture::Copy);
        settings.launch_at_login = true;
        settings
            .hotkeys
            .set(CaptureMode::Display, hotkey("Ctrl+Shift+3"))
            .unwrap();
        save(&path, &settings).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        for kept in [
            "# My settings\nfuture_setting = \"keep me\"",
            // Unchanged values keep their formatting.
            "after_capture = 'copy'  # straight to the clipboard",
            "# Hotkeys I like\n[hotkeys]",
            // A changed value keeps its comment.
            "display = \"Ctrl+Shift+3\"  # muscle memory",
            "extra = [1, 2]",
            "[plugins]\nenabled = true",
        ] {
            assert!(text.contains(kept), "{kept:?} is missing from:\n{text}");
        }
        assert!(text.contains("launch_at_login = true"), "{text}");
        assert!(!text.starts_with("# Chartreuse settings."), "{text}");
        assert_eq!(load(&path).unwrap(), settings);
    }

    #[test]
    fn saving_into_inline_and_dotted_tables_keeps_their_style() {
        let (_directory, path) = scratch();
        fs::write(
            &path,
            "hotkeys = { display = \"F1\", selection = \"F2\" }\nfile_name = \"a {date}\"\n",
        )
        .unwrap();
        let mut settings = load(&path).unwrap();
        settings
            .hotkeys
            .set(CaptureMode::Selection, hotkey("F9"))
            .unwrap();
        save(&path, &settings).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("hotkeys = { display = \"F1\", selection = \"F9\""),
            "{text}"
        );
        assert!(!text.contains("[hotkeys]"), "{text}");
        assert_eq!(load(&path).unwrap(), settings);

        fs::write(&path, "hotkeys.display = \"F1\"\n").unwrap();
        let settings = load(&path).unwrap();
        save(&path, &settings).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("hotkeys.display = \"F1\""), "{text}");
        assert_eq!(load(&path).unwrap(), settings);
    }

    #[test]
    fn invalid_toml_names_the_file_line_and_column() {
        let (_directory, path) = scratch();
        fs::write(
            &path,
            "launch_at_login = true\nfile_name = \"unterminated\n",
        )
        .unwrap();
        let message = config_error(load(&path));
        let expected = format!("{} (line 2, column ", path.display());
        assert!(message.starts_with(&expected), "{message}");
    }

    #[test]
    fn an_invalid_hotkey_names_the_key_and_the_reason() {
        let (_directory, path) = scratch();
        fs::write(
            &path,
            "[hotkeys]\nselection = \"F2\"\ndisplay = \"Ctrl+Nope\"\n",
        )
        .unwrap();
        let message = config_error(load(&path));
        assert_eq!(
            message,
            format!(
                "{} (line 3, column 11, key hotkeys.display): invalid hotkey \"Ctrl+Nope\": \
                 unknown key \"Nope\"",
                path.display()
            )
        );
    }

    #[test]
    fn invalid_values_name_their_key() {
        for (text, key) in [
            ("after_capture = \"upload\"\n", "key after_capture"),
            ("launch_at_login = \"yes\"\n", "key launch_at_login"),
            ("save_directory = \"relative\"\n", "key save_directory"),
            ("file_name = \"a/b\"\n", "key file_name"),
            (
                "[hotkeys]\nselection = \"Ctrl+Alt+Shift+3\"\n",
                "key hotkeys",
            ),
        ] {
            let message = config_error(
                parse(text)
                    .map(|parsed| parsed.settings)
                    .map_err(|problem| problem.into_error(Path::new("settings.toml"), text)),
            );
            assert!(message.contains(key), "{text:?}: {message}");
        }
    }

    #[test]
    fn non_utf8_files_are_config_errors() {
        let (_directory, path) = scratch();
        fs::write(&path, b"file_name = \"\xff\"\n").unwrap();
        let message = config_error(load(&path));
        assert!(message.contains("not a text file"), "{message}");
    }

    #[test]
    fn saving_over_an_invalid_file_leaves_it_alone() {
        let (_directory, path) = scratch();
        let broken = "launch_at_login = tru\n";
        fs::write(&path, broken).unwrap();
        let error = save(&path, &Settings::default()).unwrap_err();
        assert!(matches!(error, Error::Config(_)), "{error:?}");
        assert!(error.to_string().contains("line 1"), "{error}");
        assert_eq!(fs::read_to_string(&path).unwrap(), broken);
    }

    #[test]
    fn a_file_with_invalid_values_can_be_saved_over() {
        // Valid TOML with a bad value: saving from the settings window fixes it.
        let (_directory, path) = scratch();
        fs::write(&path, "# mine\nafter_capture = \"upload\"\n").unwrap();
        assert!(load(&path).is_err());
        save(&path, &Settings::default()).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("# mine\nafter_capture = \"open_editor\""),
            "{text}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_save_directory_that_is_not_unicode_cannot_be_saved() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;
        let (_directory, path) = scratch();
        let settings = Settings {
            save_directory: Some(
                SaveDirectory::new(Path::new(OsStr::from_bytes(b"/tmp/\xff"))).unwrap(),
            ),
            ..Settings::default()
        };
        assert!(matches!(save(&path, &settings), Err(Error::Config(_))));
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn saving_through_a_symlink_replaces_its_target() {
        let (directory, path) = scratch();
        let real = directory.path().join("dotfiles.toml");
        fs::write(&real, "# linked\n").unwrap();
        std::os::unix::fs::symlink(&real, &path).unwrap();
        save(&path, &custom()).unwrap();
        assert!(fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(load(&real).unwrap(), custom());
        assert!(fs::read_to_string(&real).unwrap().contains("# linked\n"));
    }

    #[cfg(unix)]
    #[test]
    fn saving_through_a_dangling_symlink_creates_its_target() {
        let (directory, path) = scratch();
        // Relative to the link's directory, in a folder that does not exist yet.
        std::os::unix::fs::symlink(Path::new("dotfiles").join(FILE_NAME), &path).unwrap();
        save(&path, &custom()).unwrap();
        assert!(fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            load(&directory.path().join("dotfiles").join(FILE_NAME)).unwrap(),
            custom()
        );
    }

    #[test]
    fn line_and_column_count_characters() {
        assert_eq!(line_column("abc", 0), (1, 1));
        assert_eq!(line_column("ab\ncd", 4), (2, 2));
        assert_eq!(line_column("é = 1\nx", 3), (1, 3));
    }
}
