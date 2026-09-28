//! `cargo xtask ci-keychain`: imports the release (Developer ID) signing
//! identity from CI secrets into a temporary keychain that `codesign` can use
//! without a prompt, before `cargo xtask release`.
//!
//! - [`P12_ENV`] holds the certificate and private key, exported from Keychain
//!   Access as a `.p12` file and base64-encoded (`base64 -i identity.p12`);
//!   [`P12_PASSWORD_ENV`] holds the password it was exported with.
//! - The keychain is `chartreuse-release-signing.keychain-db` in `$RUNNER_TEMP`
//!   (GitHub Actions), else the temporary folder. It gets a random password,
//!   never locks, and goes first on the user's keychain search list, where
//!   `codesign` finds the identity; its key partition list lets `codesign` use
//!   the key without asking.
//! - `security delete-keychain <path>` removes it and takes it off the search
//!   list. GitHub-hosted runners are discarded after every job; on anything
//!   else, delete it when the release is done.

use std::ffi::OsString;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use crate::macos_release::RELEASE_IDENTITY_ENV;
use crate::util::{capture, decode_base64, tool, Context, Error, PrivateDir, Result, Step};

pub const P12_ENV: &str = "CHARTREUSE_SIGN_P12_BASE64";
pub const P12_PASSWORD_ENV: &str = "CHARTREUSE_SIGN_P12_PASSWORD";

const KEYCHAIN_FILE: &str = "chartreuse-release-signing.keychain-db";
const SECURITY: &str = "/usr/bin/security";

/// The temporary keychain's path: in `runner_temp` (`$RUNNER_TEMP`, which
/// GitHub Actions empties after each job) if set, else in `temp_dir`.
#[must_use]
pub fn keychain_path(runner_temp: Option<&Path>, temp_dir: &Path) -> PathBuf {
    runner_temp
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(temp_dir)
        .join(KEYCHAIN_FILE)
}

/// The `.p12` (still base64-encoded) and its password from the environment,
/// as read by `var`. `None` if the identity is unset and `skip_if_unset`;
/// otherwise a missing variable is an error naming it.
pub fn secrets(
    var: impl Fn(&str) -> Option<String>,
    skip_if_unset: bool,
) -> Result<Option<(String, String)>> {
    let get = |name: &str| var(name).filter(|value| !value.trim().is_empty());
    match (get(P12_ENV), get(P12_PASSWORD_ENV)) {
        (Some(p12), Some(password)) => Ok(Some((p12, password))),
        (Some(_), None) => Err(Error(format!(
            "{P12_PASSWORD_ENV} is not set: set it to the password the .p12 in {P12_ENV} \
             was exported with."
        ))),
        (None, _) if skip_if_unset => Ok(None),
        (None, _) => Err(Error(format!(
            "{P12_ENV} is not set. Set it to the base64-encoded .p12 export of the \
             Developer ID Application certificate and its private key (`base64 -i \
             identity.p12`), and {P12_PASSWORD_ENV} to its password."
        ))),
    }
}

/// The keychains on the user's search list, from `security list-keychains -d
/// user` output (one quoted path per line).
#[must_use]
pub fn search_list(output: &str) -> Vec<PathBuf> {
    output
        .lines()
        .map(|line| line.trim().trim_matches('"'))
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// The commands that create `keychain`, import the identity from `p12`, and
/// put the keychain first on the search list, ahead of `search_list` (the
/// current list, which is kept).
#[must_use]
pub fn setup_steps(
    keychain: &Path,
    password: &str,
    p12: &Path,
    p12_password: &str,
    search_list: &[PathBuf],
) -> Vec<Step> {
    let mut list: Vec<OsString> = ["list-keychains", "-d", "user", "-s"]
        .map(OsString::from)
        .into();
    list.push(keychain.into());
    list.extend(
        search_list
            .iter()
            .filter(|path| path.as_path() != keychain)
            .map(OsString::from),
    );
    vec![
        Step::new(
            "create the keychain",
            SECURITY,
            &[&"create-keychain", &"-p", &password, &keychain],
        ),
        Step::new(
            "keep the keychain unlocked (no timeout, no lock on sleep)",
            SECURITY,
            &[&"set-keychain-settings", &keychain],
        ),
        Step::new(
            "unlock the keychain",
            SECURITY,
            &[&"unlock-keychain", &"-p", &password, &keychain],
        ),
        Step::new(
            "import the identity, usable by codesign",
            SECURITY,
            &[
                &"import",
                &p12,
                &"-k",
                &keychain,
                &"-f",
                &"pkcs12",
                &"-P",
                &p12_password,
                &"-T",
                &"/usr/bin/codesign",
            ],
        ),
        Step::new(
            "let codesign use the key without asking",
            SECURITY,
            &[
                &"set-key-partition-list",
                &"-S",
                &"apple-tool:,apple:,codesign:",
                &"-s",
                &"-k",
                &password,
                &keychain,
            ],
        ),
        Step {
            what: "put the keychain first on the search list",
            program: SECURITY,
            args: list,
        },
    ]
}

/// Runs a step whose arguments may hold a password, reporting a failure
/// without them.
fn run_secret(step: &Step) -> Result {
    eprintln!("ci-keychain: {}", step.what);
    let output = step
        .command()
        .output()
        .context(|| format!("could not run {}", step.program))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Error(format!(
            "could not {} ({}): {}",
            step.what,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// Whether `security find-identity` output (`  1) <SHA-1> "<name>"`) lists
/// `identity` the way `codesign --sign` finds it: by SHA-1 hash, or by part of
/// the certificate's name.
#[must_use]
pub fn lists_identity(find_identity_output: &str, identity: &str) -> bool {
    find_identity_output.lines().any(|line| {
        let name = line
            .split_once('"')
            .and_then(|(_, rest)| rest.rsplit_once('"'))
            .map(|(name, _)| name);
        name.is_some_and(|name| name.contains(identity))
            || line
                .split_whitespace()
                .nth(1)
                .is_some_and(|hash| hash.eq_ignore_ascii_case(identity))
    })
}

/// A random keychain password: nobody needs to know it, since the keychain
/// never locks.
fn random_password() -> Result<String> {
    let mut bytes = [0_u8; 24];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .context(|| "reading /dev/urandom".into())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn ci_keychain(skip_if_unset: bool) -> Result {
    let Some((p12_base64, p12_password)) = secrets(|name| std::env::var(name).ok(), skip_if_unset)?
    else {
        eprintln!("{P12_ENV} is not set: no signing identity to import, skipping");
        return Ok(());
    };
    if !cfg!(target_os = "macos") {
        return Err(Error("ci-keychain is only needed on macOS".into()));
    }
    let p12_bytes = decode_base64(P12_ENV, &p12_base64)?;
    let runner_temp = std::env::var_os("RUNNER_TEMP").map(PathBuf::from);
    let keychain = keychain_path(runner_temp.as_deref(), &std::env::temp_dir());

    let work = PrivateDir::create("chartreuse-ci-keychain")?;
    let p12 = work.path().join("identity.p12");
    std::fs::write(&p12, p12_bytes).context(|| format!("writing {}", p12.display()))?;

    if keychain.exists() {
        // Left over from an earlier run on this machine.
        let _ = capture(tool(SECURITY).arg("delete-keychain").arg(&keychain));
    }
    let listed = capture(tool(SECURITY).args(["list-keychains", "-d", "user"]))?;
    let current = search_list(&String::from_utf8_lossy(&listed.stdout));
    let password = random_password()?;
    for step in setup_steps(&keychain, &password, &p12, &p12_password, &current) {
        if let Err(error) = run_secret(&step) {
            let _ = capture(tool(SECURITY).arg("delete-keychain").arg(&keychain));
            return Err(error);
        }
    }
    drop(work);

    let found = capture(
        tool(SECURITY)
            .args(["find-identity", "-v", "-p", "codesigning"])
            .arg(&keychain),
    )?;
    let found = String::from_utf8_lossy(&found.stdout);
    eprintln!(
        "imported into {}:\n{}",
        keychain.display(),
        found.trim_end()
    );
    match std::env::var(RELEASE_IDENTITY_ENV) {
        Ok(identity) if !identity.trim().is_empty() => {
            if !lists_identity(&found, identity.trim()) {
                return Err(Error(format!(
                    "{RELEASE_IDENTITY_ENV} ({}) is not among the valid identities \
                     imported from {P12_ENV}, listed above.",
                    identity.trim()
                )));
            }
        }
        _ => eprintln!(
            "set {RELEASE_IDENTITY_ENV} to the identity's name or SHA-1 hash above for \
             `cargo xtask release`"
        ),
    }
    eprintln!(
        "When done: security delete-keychain '{}' (GitHub-hosted runners are discarded \
         after the job anyway)",
        keychain.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn from(vars: &[(&str, &str)], skip: bool) -> Result<Option<(String, String)>> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        secrets(|name| vars.get(name).cloned(), skip)
    }

    #[test]
    fn a_missing_identity_names_the_variables_unless_skipped() {
        let error = from(&[], false).unwrap_err();
        assert!(error.0.contains(P12_ENV) && error.0.contains(P12_PASSWORD_ENV));
        // An unset GitHub secret is an empty string.
        assert_eq!(from(&[(P12_ENV, "")], true).unwrap(), None);
    }

    #[test]
    fn an_identity_without_its_password_is_an_error_even_when_skipping() {
        let error = from(&[(P12_ENV, "MIIK")], true).unwrap_err();
        assert!(error.0.contains(P12_PASSWORD_ENV), "{error}");
        assert_eq!(
            from(&[(P12_ENV, "MIIK"), (P12_PASSWORD_ENV, " pass ")], false).unwrap(),
            Some(("MIIK".into(), " pass ".into()))
        );
    }

    #[test]
    fn the_keychain_goes_in_the_runner_temp_folder() {
        let temp = Path::new("/var/folders/x/T");
        assert_eq!(
            keychain_path(Some(Path::new("/Users/runner/work/_temp")), temp),
            Path::new("/Users/runner/work/_temp/chartreuse-release-signing.keychain-db")
        );
        assert_eq!(
            keychain_path(None, temp),
            Path::new("/var/folders/x/T/chartreuse-release-signing.keychain-db")
        );
    }

    #[test]
    fn the_search_list_is_read_from_security_output() {
        let output = "    \"/Users/runner/Library/Keychains/login.keychain-db\"\n    \
                      \"/Library/Keychains/System.keychain\"\n";
        assert_eq!(
            search_list(output),
            [
                PathBuf::from("/Users/runner/Library/Keychains/login.keychain-db"),
                PathBuf::from("/Library/Keychains/System.keychain"),
            ]
        );
    }

    fn steps() -> Vec<Step> {
        setup_steps(
            Path::new("/t/release.keychain-db"),
            "k3y",
            Path::new("/p/identity.p12"),
            "p12pass",
            &[
                "/Users/runner/Library/Keychains/login.keychain-db".into(),
                "/t/release.keychain-db".into(),
            ],
        )
    }

    fn step<'a>(steps: &'a [Step], subcommand: &str) -> &'a Step {
        steps
            .iter()
            .find(|step| step.args[0] == subcommand)
            .unwrap_or_else(|| panic!("no {subcommand} step"))
    }

    #[test]
    fn every_keychain_command_is_given_its_password() {
        // `security` prompts for any password it is not given, which hangs CI.
        let steps = steps();
        for (subcommand, flag) in [
            ("create-keychain", "-p"),
            ("unlock-keychain", "-p"),
            ("set-key-partition-list", "-k"),
        ] {
            let args = &step(&steps, subcommand).args;
            let at = args.iter().position(|arg| arg == flag).unwrap();
            assert_eq!(args[at + 1], "k3y", "{subcommand}");
        }
    }

    #[test]
    fn the_identity_is_imported_for_codesign_into_the_temporary_keychain() {
        let steps = steps();
        let import = &step(&steps, "import").args;
        let after = |flag: &str| {
            let at = import.iter().position(|arg| arg == flag).unwrap();
            import[at + 1].clone()
        };
        assert_eq!(import[1], "/p/identity.p12");
        assert_eq!(after("-k"), "/t/release.keychain-db");
        assert_eq!(after("-P"), "p12pass");
        assert_eq!(after("-T"), "/usr/bin/codesign");
        let partition = &step(&steps, "set-key-partition-list").args;
        assert!(partition.contains(&"apple-tool:,apple:,codesign:".into()));
        assert_eq!(partition.last().unwrap(), "/t/release.keychain-db");
    }

    #[test]
    fn the_keychain_goes_first_on_the_search_list_keeping_the_rest() {
        let steps = steps();
        assert_eq!(
            step(&steps, "list-keychains").args,
            [
                "list-keychains",
                "-d",
                "user",
                "-s",
                "/t/release.keychain-db",
                "/Users/runner/Library/Keychains/login.keychain-db"
            ]
            .map(OsString::from)
        );
        // Created before it is used.
        assert_eq!(steps[0].args[0], "create-keychain");
    }

    #[test]
    fn the_configured_identity_is_found_by_name_or_hash() {
        let output = "  1) AB76B80BF7B3F1D0FC53E1242D3F79D566418749 \"Developer ID \
                      Application: Jo (ABC123)\"\n     1 valid identities found\n";
        assert!(lists_identity(
            output,
            "Developer ID Application: Jo (ABC123)"
        ));
        assert!(lists_identity(
            output,
            "ab76b80bf7b3f1d0fc53e1242d3f79d566418749"
        ));
        // codesign matches part of the name too.
        assert!(lists_identity(output, "Developer ID Application: Jo"));
        assert!(!lists_identity(output, "Apple Development: Jo"));
        assert!(!lists_identity(output, "1)"));
    }
}
