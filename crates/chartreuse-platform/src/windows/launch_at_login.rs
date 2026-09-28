//! Windows: launch at login through the per-user `Run` key.
//!
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` holds one value per
//! program Windows starts at login. Chartreuse's is named by its bundle id
//! (so Chartreuse and Chartreuse Dev have their own) and holds the quoted
//! path of the executable that turned it on; turning it off and on again
//! points it at the running copy.
//!
//! The user can turn the entry off in Task Manager's Startup apps (or
//! Settings > Apps > Startup) without removing it: Explorer then marks it in
//! the `StartupApproved\Run` key, and [`LaunchAtLogin::status`] reports it
//! as off. Turning it on from Chartreuse removes that mark, and turning it
//! off removes both values.

use std::os::windows::ffi::OsStrExt;

use ::windows::core::PCWSTR;
use ::windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, ERROR_UNSUPPORTED_TYPE, WIN32_ERROR,
};
use ::windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_ANY,
    RRF_RT_REG_BINARY,
};
use chartreuse_core::{flavor, Error, Result};

use super::run_key::{approved, command};
use super::util::{platform_error, wide};
use crate::launch_at_login::LaunchAtLogin;

/// The per-user key whose values are the commands Windows runs at login.
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Where Explorer marks `Run` entries the user turned off (Task Manager's
/// Startup apps, or Settings > Apps > Startup), under the same value names.
const STARTUP_APPROVED_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

/// The Windows [`LaunchAtLogin`] backend.
#[derive(Debug, Default)]
pub struct WindowsLaunchAtLogin;

impl WindowsLaunchAtLogin {
    pub fn new() -> Self {
        Self
    }
}

impl LaunchAtLogin for WindowsLaunchAtLogin {
    fn status(&self) -> Result<bool> {
        let name = wide(flavor::BUNDLE_ID);
        if !value_exists(RUN_KEY, &name)? {
            return Ok(false);
        }
        Ok(approved(
            binary_value(STARTUP_APPROVED_KEY, &name)?.as_deref(),
        ))
    }

    fn set(&self, enabled: bool) -> Result<()> {
        let name = wide(flavor::BUNDLE_ID);
        if enabled {
            let exe = std::env::current_exe()
                .map_err(|error| Error::io("finding the Chartreuse executable", error))?;
            let data = command(exe.as_os_str().encode_wide());
            let subkey = wide(RUN_KEY);
            // SAFETY: the key names are NUL-terminated UTF-16 strings, and
            // `data` is the value's bytes: a NUL-terminated UTF-16 string,
            // whose byte length (under `u32::MAX` for any path) is passed.
            let status = unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    PCWSTR(subkey.as_ptr()),
                    PCWSTR(name.as_ptr()),
                    REG_SZ.0,
                    Some(data.as_ptr().cast()),
                    u32::try_from(size_of_val(data.as_slice())).unwrap_or(u32::MAX),
                )
            };
            check(
                status,
                "adding Chartreuse to the programs Windows starts at login",
            )?;
            // A mark from Task Manager would keep the entry off.
            delete_value(STARTUP_APPROVED_KEY, &name)
        } else {
            delete_value(RUN_KEY, &name)?;
            delete_value(STARTUP_APPROVED_KEY, &name)
        }
    }
}

/// Whether the value `name` (NUL-terminated UTF-16) exists under
/// `HKCU\<subkey>`.
fn value_exists(subkey: &str, name: &[u16]) -> Result<bool> {
    let subkey = wide(subkey);
    // SAFETY: the key names are NUL-terminated UTF-16 strings; with no type,
    // data, or size pointers, only the value's existence is checked.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_ANY,
            None,
            None,
            None,
        )
    };
    match status {
        ERROR_FILE_NOT_FOUND => Ok(false),
        status => check(status, "reading the programs Windows starts at login").map(|()| true),
    }
}

/// The data of the binary value `name` under `HKCU\<subkey>`; `None` if
/// there is none (or it is not binary).
fn binary_value(subkey: &str, name: &[u16]) -> Result<Option<Vec<u8>>> {
    let subkey = wide(subkey);
    let context = "reading which startup programs are turned off";
    let mut size = 0u32;
    // SAFETY: the key names are NUL-terminated UTF-16 strings; with no data
    // pointer, the value's size is written to `size`.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_BINARY,
            None,
            None,
            Some(&raw mut size),
        )
    };
    match status {
        ERROR_FILE_NOT_FOUND | ERROR_UNSUPPORTED_TYPE => return Ok(None),
        status => check(status, context)?,
    }
    let mut data = vec![0u8; size as usize];
    // SAFETY: as above, and `data` has room for the `size` bytes asked for.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_BINARY,
            None,
            Some(data.as_mut_ptr().cast()),
            Some(&raw mut size),
        )
    };
    match status {
        ERROR_FILE_NOT_FOUND | ERROR_UNSUPPORTED_TYPE => Ok(None),
        status => {
            check(status, context)?;
            data.truncate(size as usize);
            Ok(Some(data))
        }
    }
}

/// Deletes the value `name` under `HKCU\<subkey>`, if there is one.
fn delete_value(subkey: &str, name: &[u16]) -> Result<()> {
    let subkey = wide(subkey);
    // SAFETY: the key names are NUL-terminated UTF-16 strings.
    let status = unsafe {
        RegDeleteKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(name.as_ptr()),
        )
    };
    match status {
        ERROR_FILE_NOT_FOUND => Ok(()),
        status => check(
            status,
            "removing Chartreuse from the programs Windows starts at login",
        ),
    }
}

/// `Ok` for `ERROR_SUCCESS`, else an [`Error::Platform`] saying what failed.
fn check(status: WIN32_ERROR, context: &str) -> Result<()> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(platform_error(context, &status.to_hresult().into()))
    }
}
