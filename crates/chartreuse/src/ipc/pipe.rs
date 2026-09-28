//! The channel on Windows: a named pipe that only one process can own.
//!
//! The instance creates the pipe with `FILE_FLAG_FIRST_PIPE_INSTANCE`, which
//! fails while any other process has an instance of that name. It always
//! keeps one instance waiting for the next client, creating it before handing
//! a connected one out, so the name never lapses while the instance runs.
//! Named pipes vanish with their process, so nothing goes stale.
//!
//! The name is predictable, so another user's process could create it first
//! to receive this user's commands. A client opens the pipe at identification
//! level, so the server cannot act as the client, and talks to it only if it
//! runs as the client's user; otherwise [`connect`] fails and the app starts
//! without a channel.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::mem;
use std::os::windows::fs::OpenOptionsExt as _;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::time::{Duration, Instant};

use chartreuse_core::flavor;
use windows::core::HSTRING;
use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, HANDLE,
};
use windows::Win32::Security::{
    GetLengthSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId,
    WaitNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

/// The pipe's buffer sizes: a request or reply is one short line.
const BUFFER_SIZE: u32 = 4096;

/// How long [`connect`] waits while every instance of the pipe is busy.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Where the instance of one flavor listens: `\\.\pipe\<bundle id>-<user>`.
#[derive(Debug, Clone)]
pub struct Endpoint {
    path: String,
}

impl Endpoint {
    /// This user's endpoint for this build's flavor. Pipe names are shared by
    /// every session on the machine, hence the user name in it.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::NotFound`] if `USERNAME` is not set.
    pub fn current() -> io::Result<Self> {
        let user = std::env::var("USERNAME")
            .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "USERNAME is not set"))?;
        Ok(Self::named(&format!("{}-{user}", flavor::BUNDLE_ID)))
    }

    /// The pipe `\\.\pipe\<name>` (a backslash in `name` becomes `_`).
    #[must_use]
    pub fn named(name: &str) -> Self {
        Self {
            path: format!(r"\\.\pipe\{}", name.replace('\\', "_")),
        }
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.path)
    }
}

/// A connection between the command line and the instance.
#[derive(Debug)]
pub struct Stream(File);

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    /// Waits until the other end has read everything written
    /// (`FlushFileBuffers`), so closing the pipe cannot discard it.
    fn flush(&mut self) -> io::Result<()> {
        self.0.sync_all()
    }
}

/// The instance's pipe: the instance waiting for the next client.
#[derive(Debug)]
pub struct Listener {
    name: HSTRING,
    next: OwnedHandle,
}

impl Listener {
    /// Waits for the next client.
    ///
    /// # Errors
    ///
    /// `ConnectNamedPipe` failed, or the instance for the client after this
    /// one could not be created.
    pub fn accept(&mut self) -> io::Result<Stream> {
        let handle = HANDLE(self.next.as_raw_handle());
        loop {
            // SAFETY: `handle` is a pipe instance this listener owns, opened
            // without FILE_FLAG_OVERLAPPED, so the call completes synchronously.
            match unsafe { ConnectNamedPipe(handle, None) } {
                Ok(()) => break,
                // The client connected between creation and this call.
                Err(error) if error.code() == ERROR_PIPE_CONNECTED.to_hresult() => break,
                // A client connected and left before this call, such as one
                // that only checked that the instance runs. The instance takes
                // no other client until disconnected from that one.
                Err(error) if error.code() == ERROR_NO_DATA.to_hresult() => {
                    // SAFETY: as above.
                    unsafe { DisconnectNamedPipe(handle) }.map_err(io::Error::other)?;
                }
                Err(error) => return Err(io::Error::other(error)),
            }
        }
        let fresh = create_instance(&self.name, false)?;
        let connected = mem::replace(&mut self.next, fresh);
        Ok(Stream(File::from(connected)))
    }
}

/// Claims the instance: creates the pipe's first instance, unless another
/// process has the name (`None`).
///
/// # Errors
///
/// `CreateNamedPipeW` failed for another reason.
pub fn claim(endpoint: &Endpoint) -> io::Result<Option<Listener>> {
    let name = HSTRING::from(endpoint.path.as_str());
    match create_instance(&name, true) {
        Ok(next) => Ok(Some(Listener { name, next })),
        Err(error) if error.raw_os_error() == Some(win32_code(ERROR_ACCESS_DENIED.0)) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Creates an instance of the pipe `name`; the first one fails with
/// `ERROR_ACCESS_DENIED` if the name is taken.
fn create_instance(name: &HSTRING, first: bool) -> io::Result<OwnedHandle> {
    let mut open_mode = PIPE_ACCESS_DUPLEX;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    // SAFETY: `name` is a NUL-terminated wide string that outlives the call;
    // no security attributes are passed, so the default DACL applies (full
    // control for this user, read-only for others).
    let handle = unsafe {
        CreateNamedPipeW(
            name,
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            BUFFER_SIZE,
            BUFFER_SIZE,
            0,
            None,
        )
    };
    if handle.is_invalid() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the handle was just created, is valid, and nothing else owns it.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle.0) })
}

/// Connects to the instance, waiting a while if every instance of the pipe
/// is busy.
///
/// # Errors
///
/// As `CreateFileW`, see [`is_absent`]; or the process serving the pipe is
/// not this user's ([`io::ErrorKind::PermissionDenied`]) or could not be
/// looked up.
pub fn connect(endpoint: &Endpoint) -> io::Result<Stream> {
    let deadline = Instant::now() + BUSY_TIMEOUT;
    loop {
        match OpenOptions::new()
            .read(true)
            .write(true)
            // The server may identify the client but not act as it.
            .security_qos_flags(SECURITY_IDENTIFICATION.0)
            .open(&endpoint.path)
        {
            Ok(file) => {
                if user_of_server(&file)? != user_of(current_process())? {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "another user's process holds the pipe",
                    ));
                }
                allow_foreground(&file);
                return Ok(Stream(file));
            }
            Err(error)
                if error.raw_os_error() == Some(win32_code(ERROR_PIPE_BUSY.0))
                    && Instant::now() < deadline =>
            {
                let name = HSTRING::from(endpoint.path.as_str());
                // SAFETY: `name` is a NUL-terminated wide string that outlives
                // the call. A failure (the pipe went away) shows on the next
                // open.
                let _ = unsafe { WaitNamedPipeW(&name, 1000) };
            }
            Err(error) => return Err(error),
        }
    }
}

/// The user of the process serving `pipe`, the client end of a named pipe.
fn user_of_server(pipe: &File) -> io::Result<Vec<u8>> {
    let mut server = 0;
    // SAFETY: `pipe` is an open pipe handle for the duration of the call.
    unsafe { GetNamedPipeServerProcessId(HANDLE(pipe.as_raw_handle()), &mut server) }
        .map_err(io::Error::other)?;
    // SAFETY: a plain call; the handle it returns is owned right below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, server) }
        .map_err(io::Error::other)?;
    // SAFETY: the handle was just opened, is valid, and nothing else owns it.
    let process = unsafe { OwnedHandle::from_raw_handle(process.0) };
    user_of(HANDLE(process.as_raw_handle()))
}

/// Lets the process serving `pipe` bring its windows to the front, as this
/// process may: it was started by the foreground process (a terminal, or the
/// shell for a shortcut). Windows keeps a background process's new windows
/// behind the active one otherwise, so a command's overlays would not get
/// the keyboard focus, nor its editor come to the front. A failure only
/// leaves them there.
fn allow_foreground(pipe: &File) {
    let mut server = 0;
    // SAFETY: `pipe` is an open pipe handle for the duration of the call.
    let found = unsafe { GetNamedPipeServerProcessId(HANDLE(pipe.as_raw_handle()), &mut server) };
    // SAFETY: a plain call.
    if let Err(error) = found.and_then(|()| unsafe { AllowSetForegroundWindow(server) }) {
        tracing::debug!(%error, "the instance may not come to the front");
    }
}

/// This process, as a pseudo handle that needs no closing.
fn current_process() -> HANDLE {
    // SAFETY: a plain call without arguments.
    unsafe { GetCurrentProcess() }
}

/// The user `process` runs as: its token's user SID, as bytes (equal SIDs
/// have equal bytes).
fn user_of(process: HANDLE) -> io::Result<Vec<u8>> {
    let mut handle = HANDLE::default();
    // SAFETY: `process` is open with query access; the token handle returned
    // is owned right below.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut handle) }.map_err(io::Error::other)?;
    // SAFETY: the handle was just opened, is valid, and nothing else owns it.
    let token = unsafe { OwnedHandle::from_raw_handle(handle.0) };
    let mut size = 0;
    // SAFETY: `token` is open with query access. With no buffer, the call
    // only reports the size it needs (and fails for want of the buffer; a
    // real failure shows on the next call).
    let _ = unsafe { GetTokenInformation(handle, TokenUser, None, 0, &mut size) };
    // A buffer of u64s, so that the TOKEN_USER in it is aligned.
    let mut buffer = vec![0_u64; (size as usize).div_ceil(size_of::<u64>())];
    // SAFETY: `token` is open with query access, and `buffer` holds at least
    // `size` writable bytes.
    unsafe {
        GetTokenInformation(
            handle,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            size,
            &mut size,
        )
    }
    .map_err(io::Error::other)?;
    drop(token);
    // SAFETY: the call filled `buffer` with a TOKEN_USER, whose SID points
    // into `buffer` and is `GetLengthSid` bytes long.
    let sid = unsafe {
        let sid = buffer.as_ptr().cast::<TOKEN_USER>().read().User.Sid;
        std::slice::from_raw_parts(sid.0.cast::<u8>(), GetLengthSid(sid) as usize)
    };
    Ok(sid.to_vec())
}

/// Whether a [`connect`] error means nothing is listening (yet, or any more).
pub fn is_absent(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound
}

/// Pipe handles have no read or write timeouts: the instance bounds its wait
/// for the app, and a client that never writes only holds its own thread.
#[expect(
    clippy::unnecessary_wraps,
    reason = "the same signature as on Unix, where setting a timeout can fail"
)]
pub fn set_timeout(_stream: &Stream, _timeout: Duration) -> io::Result<()> {
    Ok(())
}

/// A Win32 error code as `io::Error::raw_os_error` reports it.
fn win32_code(code: u32) -> i32 {
    i32::try_from(code).expect("Win32 error codes fit in an i32")
}
