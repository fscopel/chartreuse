//! Shared helpers: errors, workspace paths, and running commands.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// An xtask failure, printed as `error: …` before exiting with status 1.
#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self(message)
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Self(message.to_owned())
    }
}

pub type Result<T = (), E = Error> = std::result::Result<T, E>;

/// Adds context to I/O errors.
pub trait Context<T> {
    fn context(self, what: impl FnOnce() -> String) -> Result<T>;
}

impl<T> Context<T> for std::io::Result<T> {
    fn context(self, what: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|error| Error(format!("{}: {error}", what())))
    }
}

/// The workspace root (the parent of this crate's directory).
pub fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the workspace")
}

/// Cargo's target directory, honoring `CARGO_TARGET_DIR`.
pub fn target_dir() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => workspace_root().join(dir),
        None => workspace_root().join("target"),
    }
}

/// The `cargo` that invoked us, so the pinned toolchain is used throughout.
pub fn cargo() -> Command {
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command.current_dir(workspace_root());
    command
}

/// A command for an external tool, run from the workspace root.
pub fn tool(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command.current_dir(workspace_root());
    command
}

fn describe(command: &Command) -> String {
    let program = Path::new(command.get_program());
    let mut text = program
        .file_name()
        .unwrap_or(program.as_os_str())
        .to_string_lossy()
        .into_owned();
    for arg in command.get_args() {
        let arg = arg.to_string_lossy();
        if arg.contains(' ') {
            text.push_str(&format!(" '{arg}'"));
        } else {
            text.push(' ');
            text.push_str(&arg);
        }
    }
    text
}

/// Runs a command with inherited stdio, failing if it does not succeed.
pub fn run(command: &mut Command) -> Result {
    eprintln!("$ {}", describe(command));
    let status = command
        .status()
        .context(|| format!("could not run {}", describe(command)))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error(format!("{} failed ({status})", describe(command))))
    }
}

/// [`run`], printing the command with every one of `secrets` (such as a
/// password among its arguments, in each form it is written in) masked.
pub fn run_redacted(command: &mut Command, secrets: &[impl AsRef<str>]) -> Result {
    let described = redact(&describe(command), secrets);
    eprintln!("$ {described}");
    let status = command
        .status()
        .context(|| format!("could not run {described}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error(format!("{described} failed ({status})")))
    }
}

/// `text` with every occurrence of each of `secrets` masked. The longest go
/// first, so a secret holding another (a password, and the password escaped)
/// is masked whole.
fn redact(text: &str, secrets: &[impl AsRef<str>]) -> String {
    let mut secrets: Vec<&str> = secrets
        .iter()
        .map(AsRef::as_ref)
        .filter(|secret| !secret.is_empty())
        .collect();
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    secrets
        .into_iter()
        .fold(text.to_owned(), |text, secret| text.replace(secret, "***"))
}

/// Runs a command and captures its output, failing if it does not succeed.
pub fn capture(command: &mut Command) -> Result<Output> {
    let output = command
        .output()
        .context(|| format!("could not run {}", describe(command)))?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(Error(format!(
            "{} failed ({}): {}",
            describe(command),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// Prints a warning that is hard to miss.
pub fn loud_warning(lines: &[&str]) {
    let rule = "=".repeat(78);
    eprintln!("warning: {rule}");
    for line in lines {
        eprintln!("warning: {line}");
    }
    eprintln!("warning: {rule}");
}

/// One command in a fixed sequence (such as `dev-cert`'s), built as data so
/// tests can check its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// What the command does, for progress output.
    pub what: &'static str,
    pub program: &'static str,
    pub args: Vec<OsString>,
}

impl Step {
    pub fn new(what: &'static str, program: &'static str, args: &[&dyn AsRef<OsStr>]) -> Self {
        Self {
            what,
            program,
            args: args.iter().map(|arg| arg.as_ref().to_owned()).collect(),
        }
    }

    pub fn command(&self) -> Command {
        let mut command = tool(self.program);
        command.args(&self.args);
        command
    }
}

/// A temporary directory only its owner can read, for key material, removed
/// with everything in it when dropped, however the command exits.
#[derive(Debug)]
pub struct PrivateDir(PathBuf);

impl PrivateDir {
    /// Creates `<temp dir>/<name>-<pid>`.
    pub fn create(name: &str) -> Result<Self> {
        let path = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder
            .create(&path)
            .context(|| format!("creating {}", path.display()))?;
        Ok(Self(path))
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Decodes standard base64 read from the environment variable `variable`
/// (named in errors), ignoring whitespace (so line-wrapped output from
/// `base64` decodes as-is).
pub fn decode_base64(variable: &str, text: &str) -> Result<Vec<u8>> {
    let invalid = |why: &str| Error(format!("{variable} is not valid base64: {why}"));
    let mut bytes = Vec::with_capacity(text.len() / 4 * 3);
    let (mut buffer, mut bits, mut digits, mut padding) = (0_u32, 0_u32, 0_usize, 0_usize);
    for c in text.bytes().filter(|c| !c.is_ascii_whitespace()) {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => {
                padding += 1;
                continue;
            }
            _ => return Err(invalid(&format!("unexpected {:?}", char::from(c)))),
        };
        if padding > 0 {
            return Err(invalid("data after the padding"));
        }
        digits += 1;
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    let padded = padding == 0 || (digits + padding) % 4 == 0;
    if digits % 4 == 1 || padding > 2 || !padded {
        return Err(invalid("truncated"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decodes_wrapped_and_padded_input() {
        let decode = |text| decode_base64("VAR", text).unwrap();
        assert_eq!(decode("TWFu"), b"Man");
        assert_eq!(decode("TWE="), b"Ma");
        assert_eq!(decode("TQ=="), b"M");
        assert_eq!(decode("TQ"), b"M");
        assert_eq!(decode("TW\nFu\r\nTQ==\n"), b"ManM");
        assert_eq!(decode("+/+/"), [0xfb, 0xff, 0xbf]);
    }

    #[test]
    fn malformed_base64_is_rejected_naming_the_variable() {
        for bad in ["T", "TWFuT", "TQ=x", "TQ===", "TW-u", "TQ="] {
            let error = decode_base64("SOME_SECRET", bad).unwrap_err();
            assert!(error.0.contains("SOME_SECRET"), "{bad}: {error}");
        }
    }

    #[test]
    fn a_redacted_command_never_shows_the_secrets() {
        let mut command = Command::new("signtool");
        command.args(["sign", "/p", "hunter 2", "/f", "a.pfx"]);
        let shown = redact(&describe(&command), &["hunter 2"]);
        assert_eq!(shown, "signtool sign /p '***' /f a.pfx");
        assert_eq!(redact("no secrets", &[""]), "no secrets");
        // A secret holding another is masked whole, whichever is given first.
        assert_eq!(redact("/p a$ /S=a$$", &["a$", "a$$"]), "/p *** /S=***");
    }
}
