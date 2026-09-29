//! The command line: `chartreuse [capture <mode> | open <file>]`. Owned by
//! track 4C.
//!
//! With no command, the process starts the resident app. With one, it hands
//! the command to the running instance and exits, non-zero if that fails; if
//! no instance is running, it starts the app and runs the command once the
//! app has booted (see [`ipc`](crate::ipc)). `--help` and `--version` print
//! and exit.
//!
//! The arguments are parsed with clap's derive API. The grammar is small, but
//! clap supplies help and version output, usage errors (exit code 2), `--`,
//! and file names that are not UTF-8, which hand parsing would have to
//! re-implement. Only the features in use are enabled, and its proc-macro
//! dependencies are ones other crates build anyway.
//!
//! # macOS
//!
//! LaunchServices (Finder, `open`, login items) starts the bundle without a
//! command, and a second LaunchServices launch only activates the app already
//! running, so those launches are single-instance already and arguments given
//! through them (`open --args`) never reach a running instance. The command
//! line is the executable inside the bundle, run directly or through a
//! symlink: `"Chartreuse Dev.app/Contents/MacOS/chartreuse" capture selection`.
//! Keep the app running (e.g. as a login item) before using it: an instance
//! started from a terminal is the terminal's child, and macOS holds the
//! terminal responsible for its Screen Recording permission.

use std::ffi::OsString;
use std::fs::File;
use std::path::PathBuf;

use chartreuse_core::capture::CaptureMode;
use chartreuse_core::{Error, Result};
use clap::{Parser, Subcommand, ValueEnum};

use crate::ipc::Command;

/// Takes and annotates screenshots.
///
/// Without a command, starts Chartreuse in the menu bar or tray. With one,
/// has the running Chartreuse carry it out, starting it first if needed.
#[derive(Debug, Parser)]
#[command(name = "chartreuse", version)]
pub struct Cli {
    #[command(subcommand)]
    pub action: Option<Action>,
}

/// A command given on the command line.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum Action {
    /// Capture the screen, then annotate the capture in an editor window
    Capture {
        /// What to capture
        #[arg(value_enum)]
        mode: Mode,
    },
    /// Open an image file in an editor window
    Open {
        /// The image file
        file: PathBuf,
    },
}

/// What `chartreuse capture` captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Mode {
    /// The whole desktop, every display
    Display,
    /// The rectangle you drag, or the window you click after pressing Space
    Selection,
}

impl From<Mode> for CaptureMode {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::Display => Self::Display,
            Mode::Selection => Self::Selection,
        }
    }
}

impl Cli {
    /// This process's command line. For `--help`, `--version`, and usage
    /// errors, prints and exits instead.
    #[must_use]
    pub fn from_env() -> Self {
        Self::parse_from(without_process_serial_number(std::env::args_os()))
    }

    /// The command to run, if any. The file to open is made absolute against
    /// the current directory (the instance has its own) and must be
    /// readable, so that a missing file fails the command line rather than
    /// only showing an alert in the app.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] if the file to open cannot be opened.
    pub fn command(self) -> Result<Option<Command>> {
        match self.action {
            None => Ok(None),
            Some(Action::Capture { mode }) => Ok(Some(Command::Capture(mode.into()))),
            Some(Action::Open { file }) => {
                let opening = |error| Error::io(format!("opening {}", file.display()), error);
                let path = std::path::absolute(&file).map_err(opening)?;
                File::open(&path).map_err(opening)?;
                Ok(Some(Command::Open(path)))
            }
        }
    }
}

/// `args` without a `-psn_…` process serial number right after the program
/// name, which LaunchServices passes to apps on some macOS versions.
fn without_process_serial_number(
    args: impl IntoIterator<Item = OsString>,
) -> impl Iterator<Item = OsString> {
    args.into_iter()
        .enumerate()
        .filter(|(index, arg)| {
            *index != 1 || !arg.to_str().is_some_and(|arg| arg.starts_with("-psn_"))
        })
        .map(|(_, arg)| arg)
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;

    use super::*;

    fn parse(args: &[&str]) -> std::result::Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("chartreuse").chain(args.iter().copied()))
    }

    fn command(args: &[&str]) -> Option<Command> {
        parse(args).unwrap().command().unwrap()
    }

    #[test]
    fn no_command_starts_the_app() {
        assert_eq!(command(&[]), None);
    }

    #[test]
    fn capture_takes_each_mode() {
        for (word, mode) in [
            ("display", CaptureMode::Display),
            ("selection", CaptureMode::Selection),
        ] {
            assert_eq!(
                command(&["capture", word]),
                Some(Command::Capture(mode)),
                "{word}"
            );
        }
    }

    #[test]
    fn open_takes_the_file_relative_to_the_current_directory() {
        // Tests run in the crate's directory.
        let expected = std::env::current_dir().unwrap().join("Cargo.toml");
        assert_eq!(
            command(&["open", "Cargo.toml"]),
            Some(Command::Open(expected.clone()))
        );
        assert_eq!(
            command(&["open", "--", expected.to_str().unwrap()]),
            Some(Command::Open(expected))
        );
    }

    #[test]
    fn a_file_that_cannot_be_opened_fails_on_the_command_line() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing.png");
        let error = parse(&["open", missing.to_str().unwrap()])
            .unwrap()
            .command()
            .unwrap_err();
        assert!(matches!(error, Error::Io { .. }), "{error:?}");
        assert!(error.to_string().contains("missing.png"), "{error}");
    }

    #[test]
    fn usage_errors_are_reported_with_exit_code_2() {
        for args in [
            &["capture"][..],
            &["capture", "screen"],
            &["capture", "display", "window"],
            &["open"],
            &["open", "a.png", "b.png"],
            &["paste"],
            &["--quiet"],
        ] {
            let error = parse(args).expect_err(&format!("{args:?} should not parse"));
            assert_eq!(error.exit_code(), 2, "{args:?}: {error}");
        }
    }

    #[test]
    fn help_and_version_are_printed() {
        for args in [
            &["--help"][..],
            &["-h"],
            &["help"],
            &["help", "capture"],
            &["capture", "--help"],
        ] {
            let error = parse(args).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::DisplayHelp, "{args:?}");
            assert_eq!(error.exit_code(), 0);
        }
        let help = parse(&["--help"]).unwrap_err().to_string();
        assert!(
            help.contains("capture") && help.contains("open"),
            "the help lists the commands:\n{help}"
        );

        for args in [&["--version"][..], &["-V"]] {
            let error = parse(args).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::DisplayVersion, "{args:?}");
            assert!(error.to_string().contains(env!("CARGO_PKG_VERSION")));
        }
    }

    #[test]
    fn a_launch_services_serial_number_is_ignored() {
        let args = |args: &[&str]| -> Vec<OsString> {
            without_process_serial_number(args.iter().map(OsString::from)).collect()
        };
        assert_eq!(args(&["chartreuse", "-psn_0_12345"]), ["chartreuse"]);
        assert_eq!(
            args(&["chartreuse", "capture", "-psn_0_12345"]),
            ["chartreuse", "capture", "-psn_0_12345"],
            "only right after the program name"
        );
        assert_eq!(args(&["chartreuse", "capture"]), ["chartreuse", "capture"]);
    }
}
