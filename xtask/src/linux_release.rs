//! `cargo xtask release` on Linux: the release-flavor executable with the
//! license, the readme, and a `share/` tree to copy into `~/.local/share` or
//! `/usr/share` ([`write_share`]: the desktop entry, the AppStream metadata,
//! and the app icon in the freedesktop `hicolor` layout), in a `.tar.gz`.

use std::path::{Path, PathBuf};

use chartreuse_core::flavor::Flavor;

use crate::icon;
use crate::release::{archive_dir, archive_stem, build_executable, stage, VERSION};
use crate::util::{workspace_root, Context, Result};

/// The desktop entry, relative to the workspace root; installed as
/// `share/applications/<app ID>.desktop`.
pub const DESKTOP_ENTRY: &str = "packaging/linux/io.jennings.chartreuse.desktop";
/// The AppStream metadata, relative to the workspace root; installed as
/// `share/metainfo/<app ID>.metainfo.xml`.
pub const METAINFO: &str = "packaging/linux/io.jennings.chartreuse.metainfo.xml";

/// Writes the release's freedesktop data files into `share`: the desktop
/// entry, the AppStream metadata, and the app icon, all named for the app ID.
pub fn write_share(share: &Path) -> Result {
    let flavor = Flavor::Release;
    let id = flavor.bundle_id();
    for (from, to) in [
        (
            DESKTOP_ENTRY,
            share.join("applications").join(format!("{id}.desktop")),
        ),
        (
            METAINFO,
            share.join("metainfo").join(format!("{id}.metainfo.xml")),
        ),
    ] {
        let from = workspace_root().join(from);
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir).context(|| format!("creating {}", dir.display()))?;
        }
        std::fs::copy(&from, &to).context(|| format!("copying {}", from.display()))?;
    }
    icon::write_linux_icons(flavor.accent(), &share.join("icons").join("hicolor"), id)
}

pub fn release(dist: &Path, extension: &str) -> Result<Vec<PathBuf>> {
    let executable = build_executable()?;
    let stem = archive_stem(VERSION, "linux", std::env::consts::ARCH, false);
    let staging = stage(&stem, &executable)?;
    write_share(&staging.join("share"))?;
    let archive = dist.join(format!("{stem}.{extension}"));
    archive_dir(&staging, &archive)?;
    Ok(vec![archive])
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use quick_xml::events::Event;

    use super::*;
    use crate::bundle::EXECUTABLE;

    const APP_ID: &str = Flavor::Release.bundle_id();

    /// A desktop entry's groups, each a map of its keys, failing on anything
    /// `desktop-file-validate` rejects that a typo could introduce: a line
    /// outside a group, a line that is not `key=value`, or a repeated group
    /// or key.
    fn parse_desktop_entry(text: &str) -> BTreeMap<&str, BTreeMap<&str, &str>> {
        let mut groups: BTreeMap<&str, BTreeMap<&str, &str>> = BTreeMap::new();
        let mut group = None;
        for line in text.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                assert!(
                    groups.insert(name, BTreeMap::new()).is_none(),
                    "repeated [{name}]"
                );
                group = Some(name);
                continue;
            }
            let group = group.unwrap_or_else(|| panic!("{line:?} is outside a group"));
            let (key, value) = line
                .split_once('=')
                .unwrap_or_else(|| panic!("{line:?} is not key=value"));
            assert_eq!(key, key.trim(), "{line:?}");
            let keys = groups.get_mut(group).unwrap();
            assert!(
                keys.insert(key, value).is_none(),
                "repeated {key} in [{group}]"
            );
        }
        groups
    }

    #[test]
    fn the_desktop_entry_launches_the_app_with_its_icon() {
        let text = std::fs::read_to_string(workspace_root().join(DESKTOP_ENTRY)).unwrap();
        let groups = parse_desktop_entry(&text);
        let entry = &groups["Desktop Entry"];
        assert_eq!(entry["Type"], "Application");
        assert_eq!(entry["Name"], Flavor::Release.display_name());
        assert_eq!(entry["Exec"], EXECUTABLE);
        // The icon, like the file names, is the app ID that Flatpak exports
        // and that `write_linux_icons` names the PNGs for.
        assert_eq!(entry["Icon"], APP_ID);
        assert_eq!(
            Path::new(DESKTOP_ENTRY).file_name().unwrap(),
            format!("{APP_ID}.desktop").as_str()
        );
        for key in ["Categories", "Keywords", "Actions"] {
            assert!(entry[key].ends_with(';'), "{key} is a ;-terminated list");
        }
        assert!(
            entry["Categories"]
                .split(';')
                .any(|c| ["Graphics", "Utility"].contains(&c)),
            "a main category"
        );
    }

    #[test]
    fn every_desktop_action_runs_a_chartreuse_command() {
        let text = std::fs::read_to_string(workspace_root().join(DESKTOP_ENTRY)).unwrap();
        let groups = parse_desktop_entry(&text);
        let actions: Vec<&str> = groups["Desktop Entry"]["Actions"]
            .split(';')
            .filter(|action| !action.is_empty())
            .collect();
        assert!(!actions.is_empty());
        for action in &actions {
            let group = &groups[format!("Desktop Action {action}").as_str()];
            assert!(!group["Name"].is_empty(), "{action}");
            let exec: Vec<&str> = group["Exec"].split(' ').collect();
            assert_eq!(exec[..2], [EXECUTABLE, "capture"], "{action}");
            assert_eq!(format!("capture-{}", exec[2]), *action);
        }
        let action_groups = groups.keys().filter(|g| g.starts_with("Desktop Action "));
        assert_eq!(
            action_groups.count(),
            actions.len(),
            "no group without an action"
        );
    }

    /// Every element of an XML document as its `/`-separated path, its
    /// attributes, and its text; parsing fails on malformed XML, such as a
    /// mismatched end tag.
    fn parse_xml(text: &str) -> Vec<(String, BTreeMap<String, String>, String)> {
        let mut reader = quick_xml::Reader::from_str(text);
        let (mut path, mut open, mut elements) = (Vec::new(), Vec::new(), Vec::new());
        loop {
            let event = reader.read_event().expect("well-formed XML");
            let (start, empty) = match &event {
                Event::Start(start) => (Some(start.clone()), false),
                Event::Empty(start) => (Some(start.clone()), true),
                Event::Text(text) => {
                    if let Some(&index) = open.last() {
                        let element: &mut (String, BTreeMap<String, String>, String) =
                            &mut elements[index];
                        element.2.push_str(AsRef::<str>::as_ref(text).trim());
                    }
                    continue;
                }
                Event::End(_) => {
                    path.pop();
                    open.pop();
                    continue;
                }
                Event::Eof => break,
                _ => continue,
            };
            let start = start.unwrap();
            let name = start.name().as_ref().to_owned();
            let attributes = start
                .attributes()
                .map(|attribute| {
                    let attribute = attribute.unwrap();
                    let key = attribute.key.as_ref().to_owned();
                    (
                        key,
                        attribute
                            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            .unwrap()
                            .into_owned(),
                    )
                })
                .collect();
            path.push(name);
            elements.push((path.join("/"), attributes, String::new()));
            if empty {
                path.pop();
            } else {
                open.push(elements.len() - 1);
            }
        }
        assert!(path.is_empty(), "every element is closed");
        elements
    }

    #[test]
    fn the_metainfo_describes_this_app_and_version() {
        let text = std::fs::read_to_string(workspace_root().join(METAINFO)).unwrap();
        let elements = parse_xml(&text);
        let text_of = |path: &str| {
            let found: Vec<&str> = elements
                .iter()
                .filter(|(p, _, _)| p == path)
                .map(|(_, _, text)| text.as_str())
                .collect();
            assert_eq!(found.len(), 1, "one {path}");
            found[0]
        };
        assert_eq!(
            Path::new(METAINFO).file_name().unwrap(),
            format!("{APP_ID}.metainfo.xml").as_str()
        );
        assert_eq!(elements[0].0, "component");
        assert_eq!(elements[0].1["type"], "desktop-application");
        assert_eq!(text_of("component/id"), APP_ID);
        assert_eq!(text_of("component/name"), Flavor::Release.display_name());
        assert_eq!(text_of("component/launchable"), format!("{APP_ID}.desktop"));
        assert_eq!(text_of("component/provides/binary"), EXECUTABLE);
        assert_eq!(
            text_of("component/project_license"),
            env!("CARGO_PKG_LICENSE")
        );
        assert!(
            !text_of("component/summary").ends_with('.'),
            "AppStream: no period"
        );
    }

    #[test]
    fn the_newest_metainfo_release_is_this_version() {
        let text = std::fs::read_to_string(workspace_root().join(METAINFO)).unwrap();
        let releases: Vec<_> = parse_xml(&text)
            .into_iter()
            .filter(|(path, _, _)| path == "component/releases/release")
            .map(|(_, attributes, _)| attributes)
            .collect();
        assert_eq!(
            releases.first().map(|release| release["version"].as_str()),
            Some(VERSION),
            "add a <release> for {VERSION}, newest first, to {METAINFO}"
        );
        for release in &releases {
            let date = &release["date"];
            let digits: Vec<&str> = date.split('-').collect();
            assert!(
                digits.len() == 3
                    && date.len() == 10
                    && date.chars().all(|c| c == '-' || c.is_ascii_digit()),
                "{date} is YYYY-MM-DD"
            );
        }
    }
}
