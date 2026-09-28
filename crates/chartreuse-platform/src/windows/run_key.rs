//! Windows: the `Run` key entry that starts Chartreuse at login (the portable
//! part of `launch_at_login.rs`).

/// The `Run` value's data for the executable at `exe` (UTF-16): the path in
/// quotes, so Windows does not split it at a space, then the terminating NUL
/// of a `REG_SZ`. Windows paths cannot contain `"`, so nothing needs escaping.
pub(super) fn command(exe: impl IntoIterator<Item = u16>) -> Vec<u16> {
    let quote = u16::from(b'"');
    std::iter::once(quote)
        .chain(exe)
        .chain([quote, 0])
        .collect()
}

/// Whether Explorer lets a `Run` entry start, given its `StartupApproved`
/// value (`None` if there is none, as for an entry the user never touched).
/// The value's first byte is even (2 or 6) when the entry is on and odd (3 or
/// 7) when the user turned it off; a timestamp of that follows.
pub(super) fn approved(entry: Option<&[u8]>) -> bool {
    entry
        .and_then(|data| data.first())
        .is_none_or(|flags| flags & 1 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_quotes_the_path_and_ends_in_nul() {
        let exe = r"C:\Program Files\Chartreuse\chartreuse.exe";
        let expected: Vec<u16> = format!("\"{exe}\"\0").encode_utf16().collect();
        assert_eq!(command(exe.encode_utf16()), expected);
    }

    #[test]
    fn the_command_keeps_paths_that_are_not_valid_unicode() {
        // An unpaired surrogate, which NTFS allows in file names.
        let exe = [u16::from(b'C'), 0xD800];
        assert_eq!(
            command(exe),
            [u16::from(b'"'), u16::from(b'C'), 0xD800, u16::from(b'"'), 0]
        );
    }

    #[test]
    fn entries_start_unless_the_user_turned_them_off() {
        assert!(approved(None));
        assert!(approved(Some(&[])));
        assert!(approved(Some(&[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])));
        assert!(approved(Some(&[6, 0, 0, 0])));
        assert!(!approved(Some(&[
            3, 0, 0, 0, 0x40, 0x1d, 0x8c, 0x5e, 0x2c, 0xf4, 0xd8, 0x01
        ])));
        assert!(!approved(Some(&[7, 0, 0, 0])));
    }
}
