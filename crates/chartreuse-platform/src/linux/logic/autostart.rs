//! XDG autostart entries (the Desktop Application Autostart Specification):
//! where Chartreuse's goes, what it holds, and whether it is turned on.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The autostart entry of the app with `bundle_id`:
/// `$XDG_CONFIG_HOME/autostart/<bundle id>.desktop`, where an unset, empty,
/// or relative `XDG_CONFIG_HOME` means `$HOME/.config`. `None` if neither
/// gives an absolute folder.
pub fn entry_path(
    xdg_config_home: Option<&OsStr>,
    home: Option<&OsStr>,
    bundle_id: &str,
) -> Option<PathBuf> {
    fn absolute(value: Option<&OsStr>) -> Option<&Path> {
        value.map(Path::new).filter(|path| path.is_absolute())
    }
    let config = absolute(xdg_config_home)
        .map(Path::to_path_buf)
        .or_else(|| absolute(home).map(|home| home.join(".config")))?;
    Some(
        config
            .join("autostart")
            .join(format!("{bundle_id}.desktop")),
    )
}

/// The contents of an autostart entry that starts the executable at `exe`,
/// shown as `name`.
pub fn entry(name: &str, exe: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={}\n\
         Exec={}\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        escape(name),
        escape(&exec_argument(exe)),
    )
}

/// `argument` quoted for an `Exec` key, before the escaping of string
/// values: in double quotes, with `"`, `` ` ``, `$`, and `\` escaped by a
/// backslash, and `%` doubled so it is not a field code.
fn exec_argument(argument: &str) -> String {
    let mut quoted = String::with_capacity(argument.len() + 2);
    quoted.push('"');
    for c in argument.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// `value` escaped as a desktop entry string value.
fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => escaped.push_str(r"\\"),
            '\n' => escaped.push_str(r"\n"),
            '\t' => escaped.push_str(r"\t"),
            '\r' => escaped.push_str(r"\r"),
            c => escaped.push(c),
        }
    }
    escaped
}

/// Whether an autostart entry holding `contents` starts its program: it does
/// unless its `[Desktop Entry]` group sets `Hidden=true` (the specification's
/// way to turn an entry off) or `X-GNOME-Autostart-enabled=false` (what
/// GNOME's and some other desktops' startup settings write).
pub fn enabled(contents: &str) -> bool {
    let mut in_entry = false;
    for line in contents.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match (key.trim_end(), value.trim_start()) {
            ("Hidden", "true") | ("X-GNOME-Autostart-enabled", "false") => return false,
            _ => {}
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(xdg_config_home: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
        entry_path(
            xdg_config_home.map(OsStr::new),
            home.map(OsStr::new),
            "io.jennings.chartreuse",
        )
    }

    #[test]
    fn the_entry_goes_in_the_config_home_or_else_under_home() {
        let expected = |folder: &str| {
            Some(PathBuf::from(folder).join("autostart/io.jennings.chartreuse.desktop"))
        };
        assert_eq!(path(Some("/cfg"), Some("/home/u")), expected("/cfg"));
        for ignored in [None, Some(""), Some("relative/cfg")] {
            assert_eq!(path(ignored, Some("/home/u")), expected("/home/u/.config"));
        }
        assert_eq!(path(None, None), None);
        assert_eq!(path(Some("cfg"), Some("home")), None);
    }

    #[test]
    fn the_entry_starts_the_quoted_executable() {
        let entry = entry("Chartreuse Dev", "/opt/My Apps/chartreuse");
        assert!(entry.starts_with("[Desktop Entry]\n"), "{entry}");
        assert!(entry.contains("\nType=Application\n"), "{entry}");
        assert!(entry.contains("\nName=Chartreuse Dev\n"), "{entry}");
        assert!(
            entry.contains("\nExec=\"/opt/My Apps/chartreuse\"\n"),
            "{entry}"
        );
        assert!(enabled(&entry));
    }

    #[test]
    fn reserved_characters_in_the_path_are_escaped() {
        let exec = |exe: &str| {
            entry("Chartreuse", exe)
                .lines()
                .find_map(|line| line.strip_prefix("Exec="))
                .unwrap()
                .to_owned()
        };
        assert_eq!(exec(r#"/a"b`c$d"#), r#""/a\\"b\\`c\\$d""#);
        assert_eq!(exec("/100%/x"), r#""/100%%/x""#);
        // A literal backslash takes four: one escape per rule.
        assert_eq!(exec(r"/a\b"), r#""/a\\\\b""#);
        assert_eq!(exec("/a\nb"), r#""/a\nb""#);
    }

    #[test]
    fn entries_turned_off_do_not_start() {
        let entry = entry("Chartreuse", "/usr/bin/chartreuse");
        assert!(!enabled(&format!("{entry}Hidden=true\n")));
        assert!(!enabled(&entry.replace(
            "X-GNOME-Autostart-enabled=true",
            "X-GNOME-Autostart-enabled = false"
        )));
        assert!(enabled(&format!("{entry}Hidden=false\n")));
        // Only the main group counts.
        assert!(enabled(&format!(
            "{entry}[Desktop Action other]\nHidden=true\n"
        )));
        assert!(enabled(&format!("{entry}# Hidden=true\n")));
    }
}
