//! The wire format between the command line and the running instance.
//!
//! A client connects, writes one request line, and reads one reply line; then
//! both sides close the connection. A line ends in `\n` and holds at most
//! [`MAX_LINE`] bytes before it.
//!
//! | Request | Command |
//! |---|---|
//! | `capture display`, `capture selection` | [`Command::Capture`] |
//! | `open <absolute path>` | [`Command::Open`] |
//!
//! The reply is `ok` once the instance has taken the command, or
//! `error <message>` if it refused it.
//!
//! In a path or a message, `\` is written `\\` and a line break `\n`, so a
//! line always holds exactly one request or reply. Paths are sent as their raw
//! bytes on Unix (file names need not be UTF-8) and as UTF-8 on Windows.
//!
//! A connection that closes without a request is not an error: it only
//! checked that the instance is there.

use std::fmt;
use std::io::{self, BufRead, Read as _};
use std::path::{Path, PathBuf};

use chartreuse_core::capture::CaptureMode;

use super::Command;

/// The longest line either side accepts, without its `\n`: room for the
/// longest paths, even with every byte escaped.
pub const MAX_LINE: usize = 64 * 1024;

/// A request or reply that does not follow the protocol, or a command that
/// cannot be written in it. The text says what is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolError(String);

impl ProtocolError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProtocolError {}

/// The request line for `command`, `\n` included.
///
/// # Errors
///
/// On Windows, a path that is not valid Unicode.
pub fn encode_request(command: &Command) -> Result<Vec<u8>, ProtocolError> {
    let mut line = Vec::new();
    match command {
        Command::Capture(mode) => {
            line.extend_from_slice(b"capture ");
            line.extend_from_slice(mode_word(*mode).as_bytes());
        }
        Command::Open(path) => {
            line.extend_from_slice(b"open ");
            escape_into(path_bytes(path)?, &mut line);
        }
    }
    line.push(b'\n');
    Ok(line)
}

/// The command of a request `line` (without its `\n`).
///
/// # Errors
///
/// An unknown request or capture mode, a missing argument, a malformed escape,
/// or a path that is not absolute (the instance's working directory is not
/// the client's).
pub fn decode_request(line: &[u8]) -> Result<Command, ProtocolError> {
    match split(line) {
        (b"capture", Some(word)) => mode_from_word(word).map(Command::Capture).ok_or_else(|| {
            ProtocolError::new(format!(
                "unknown capture mode “{}”",
                String::from_utf8_lossy(word)
            ))
        }),
        (b"open", Some(escaped)) => {
            let path = path_from_bytes(unescape(escaped)?)?;
            if path.is_absolute() {
                Ok(Command::Open(path))
            } else {
                Err(ProtocolError::new(format!(
                    "the path to open must be absolute, not “{}”",
                    path.display()
                )))
            }
        }
        _ => Err(ProtocolError::new(format!(
            "unknown request “{}”",
            String::from_utf8_lossy(line)
        ))),
    }
}

/// The reply line for `reply`, `\n` included.
#[must_use]
pub fn encode_reply(reply: &Result<(), String>) -> Vec<u8> {
    match reply {
        Ok(()) => b"ok\n".to_vec(),
        Err(message) => {
            let mut line = b"error ".to_vec();
            escape_into(message.as_bytes(), &mut line);
            line.push(b'\n');
            line
        }
    }
}

/// The reply in a reply `line` (without its `\n`): `Ok` for `ok`, the
/// message for `error <message>`.
///
/// # Errors
///
/// Anything else.
pub fn decode_reply(line: &[u8]) -> Result<Result<(), String>, ProtocolError> {
    match split(line) {
        (b"ok", None) => Ok(Ok(())),
        (b"error", Some(escaped)) => Ok(Err(
            String::from_utf8_lossy(&unescape(escaped)?).into_owned()
        )),
        _ => Err(ProtocolError::new(format!(
            "unexpected reply “{}”",
            String::from_utf8_lossy(line)
        ))),
    }
}

/// Reads one line, without its `\n`; `None` if the stream ends before it
/// starts.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidData`] for a line longer than [`MAX_LINE`],
/// [`io::ErrorKind::UnexpectedEof`] for one cut off by the end of the stream,
/// and the reader's own errors.
pub fn read_line(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    const LIMIT: u64 = MAX_LINE as u64 + 1;
    let mut line = Vec::new();
    let read = reader.by_ref().take(LIMIT).read_until(b'\n', &mut line)?;
    if read == 0 {
        return Ok(None);
    }
    if line.last() == Some(&b'\n') {
        line.pop();
        Ok(Some(line))
    } else if read as u64 == LIMIT {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the line is longer than {MAX_LINE} bytes"),
        ))
    } else {
        Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the connection closed in the middle of a line",
        ))
    }
}

/// The word for `mode` in a `capture` request.
pub(super) const fn mode_word(mode: CaptureMode) -> &'static str {
    match mode {
        CaptureMode::Display => "display",
        CaptureMode::Selection => "selection",
    }
}

fn mode_from_word(word: &[u8]) -> Option<CaptureMode> {
    CaptureMode::ALL
        .into_iter()
        .find(|&mode| mode_word(mode).as_bytes() == word)
}

/// Splits a line into its first word and, after the first space, the rest.
fn split(line: &[u8]) -> (&[u8], Option<&[u8]>) {
    match line.iter().position(|&byte| byte == b' ') {
        Some(space) => (&line[..space], Some(&line[space + 1..])),
        None => (line, None),
    }
}

fn escape_into(bytes: &[u8], line: &mut Vec<u8>) {
    for &byte in bytes {
        match byte {
            b'\\' => line.extend_from_slice(b"\\\\"),
            b'\n' => line.extend_from_slice(b"\\n"),
            _ => line.push(byte),
        }
    }
}

fn unescape(escaped: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    let mut bytes = Vec::with_capacity(escaped.len());
    let mut rest = escaped.iter();
    while let Some(&byte) = rest.next() {
        if byte != b'\\' {
            bytes.push(byte);
            continue;
        }
        match rest.next() {
            Some(b'\\') => bytes.push(b'\\'),
            Some(b'n') => bytes.push(b'\n'),
            _ => return Err(ProtocolError::new("malformed escape (only \\\\ and \\n)")),
        }
    }
    Ok(bytes)
}

#[cfg(unix)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "every Unix path has bytes; Windows paths may not be Unicode"
)]
fn path_bytes(path: &Path) -> Result<&[u8], ProtocolError> {
    use std::os::unix::ffi::OsStrExt as _;
    Ok(path.as_os_str().as_bytes())
}

#[cfg(unix)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "any bytes are a Unix path; Windows paths are UTF-8 on the wire"
)]
fn path_from_bytes(bytes: Vec<u8>) -> Result<PathBuf, ProtocolError> {
    use std::os::unix::ffi::OsStringExt as _;
    Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

#[cfg(windows)]
fn path_bytes(path: &Path) -> Result<&[u8], ProtocolError> {
    path.to_str()
        .map(str::as_bytes)
        .ok_or_else(|| ProtocolError::new(format!("“{}” is not valid Unicode", path.display())))
}

#[cfg(windows)]
fn path_from_bytes(bytes: Vec<u8>) -> Result<PathBuf, ProtocolError> {
    String::from_utf8(bytes)
        .map(PathBuf::from)
        .map_err(|_| ProtocolError::new("the path is not valid UTF-8"))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    /// An absolute path on this platform.
    fn absolute(path: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!(r"C:\{}", path.replace('/', r"\")))
        } else {
            PathBuf::from(format!("/{path}"))
        }
    }

    /// `line` without its `\n`.
    fn body(line: &[u8]) -> &[u8] {
        line.strip_suffix(b"\n").expect("lines end in \\n")
    }

    fn round_trip(command: &Command) -> Command {
        decode_request(body(&encode_request(command).unwrap())).unwrap()
    }

    #[test]
    fn requests_are_the_documented_lines() {
        // Other clients (e.g. `printf 'capture selection\n' | nc -U …`) rely on these.
        let capture = |mode| encode_request(&Command::Capture(mode)).unwrap();
        assert_eq!(capture(CaptureMode::Display), b"capture display\n");
        assert_eq!(capture(CaptureMode::Selection), b"capture selection\n");
        assert_eq!(encode_reply(&Ok(())), b"ok\n");
        assert_eq!(encode_reply(&Err("no".into())), b"error no\n");
    }

    #[test]
    fn every_command_survives_the_trip() {
        for mode in CaptureMode::ALL {
            assert_eq!(round_trip(&Command::Capture(mode)), Command::Capture(mode));
        }
        for path in [
            "shots/plain.png",
            "with spaces/and\\backslashes.png",
            "line\nbreak/and a trailing backslash\\",
            "literal \\n is not a line break.png",
            "ünïcødé/截图.png",
        ] {
            let open = Command::Open(absolute(path));
            let line = encode_request(&open).unwrap();
            assert_eq!(
                line.iter().filter(|&&byte| byte == b'\n').count(),
                1,
                "{path:?} fits on one line"
            );
            assert_eq!(round_trip(&open), open, "{path:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_file_names_need_not_be_utf8() {
        use std::os::unix::ffi::OsStringExt as _;
        let path = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/caf\xe9.png".to_vec()));
        let open = Command::Open(path);
        assert_eq!(round_trip(&open), open);
    }

    #[test]
    fn malformed_requests_are_refused() {
        for line in [
            &b""[..],
            b"capture",
            b"capture screen",
            b"capture display extra",
            b"Capture display",
            b"open",
            b"open ",
            b"open relative/path.png",
            b"paste",
        ] {
            assert!(
                decode_request(line).is_err(),
                "{:?} should be refused",
                String::from_utf8_lossy(line)
            );
        }
        let mut bad_escape = b"open ".to_vec();
        bad_escape.extend_from_slice(absolute("a").as_os_str().as_encoded_bytes());
        bad_escape.extend_from_slice(b"\\t.png");
        assert!(decode_request(&bad_escape).is_err());
    }

    #[test]
    fn replies_survive_the_trip() {
        for reply in [
            Ok(()),
            Err("a capture is already in progress".to_owned()),
            Err("two\nlines and a \\".to_owned()),
            Err(String::new()),
        ] {
            let line = encode_reply(&reply);
            assert_eq!(decode_reply(body(&line)), Ok(reply));
        }
        for line in [
            &b""[..],
            b"OK",
            b"ok then",
            b"error",
            b"error \\x",
            b"maybe",
        ] {
            assert!(decode_reply(line).is_err(), "{line:?}");
        }
    }

    #[test]
    fn lines_are_read_one_at_a_time() {
        let mut reader = Cursor::new(b"capture selection\nok\n\n".to_vec());
        assert_eq!(
            read_line(&mut reader).unwrap().unwrap(),
            b"capture selection"
        );
        assert_eq!(read_line(&mut reader).unwrap().unwrap(), b"ok");
        assert_eq!(read_line(&mut reader).unwrap().unwrap(), b"");
        assert_eq!(
            read_line(&mut reader).unwrap(),
            None,
            "the end of the stream"
        );
    }

    #[test]
    fn cut_off_and_overlong_lines_are_errors() {
        let cut_off = read_line(&mut Cursor::new(b"capture win".to_vec())).unwrap_err();
        assert_eq!(cut_off.kind(), io::ErrorKind::UnexpectedEof);

        let mut longest = vec![b'x'; MAX_LINE];
        longest.push(b'\n');
        let read = read_line(&mut Cursor::new(longest)).unwrap().unwrap();
        assert_eq!(read.len(), MAX_LINE);

        let mut overlong = vec![b'x'; MAX_LINE + 1];
        overlong.push(b'\n');
        let error = read_line(&mut Cursor::new(overlong)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
