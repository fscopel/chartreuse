//! `cargo xtask dev-cert`: a signing identity that keeps Screen Recording grants
//! across rebuilds.
//!
//! macOS keys a Screen Recording grant on the designated requirement of the build
//! that first asked for it. An ad-hoc build's requirement is its `cdhash`, so every
//! rebuild, in every workspace, is a different app to macOS: the grant stops
//! applying, and each launch that asks prompts again.
//!
//! This command creates a self-signed code-signing certificate and its private key
//! in a keychain of their own, `~/Library/Keychains/chartreuse-dev-signing.keychain-db`.
//! When `CHARTREUSE_SIGN_IDENTITY` is unset, `bundle` signs with it. The designated
//! requirement is then `identifier "io.jennings.chartreuse.dev" and certificate
//! leaf = H"…"`, which every build signed this way satisfies.
//!
//! Nothing here prompts or touches the user's own keychains:
//!
//! - The keychain has a fixed password, [`KEYCHAIN_PASSWORD`]. It is not a secret;
//!   the keychain holds nothing but this identity.
//! - `codesign` is on the key's access list and partition list, so signing never
//!   asks for permission to use the key.
//! - The keychain is never added to the search list or made the default. Every
//!   `security` and `codesign` call names it explicitly.
//! - The certificate is not trusted, and nothing needs it to be.
//!   `security find-identity -v` leaves it out and Gatekeeper would reject it,
//!   but `codesign` signs with it and TCC matches the requirement.
//!
//! `/usr/bin/openssl` (LibreSSL) writes PKCS #12 files that `security import`
//! reads. OpenSSL 3, as installed by Homebrew, uses defaults that it cannot read.

use std::path::{Path, PathBuf};

use chartreuse_core::flavor::Flavor;

use crate::sign::Identity;
use crate::util::{capture, tool, Context, Error, PrivateDir, Result, Step};

/// The keychain's file name in `~/Library/Keychains`.
const KEYCHAIN_FILE: &str = "chartreuse-dev-signing.keychain-db";
/// The keychain's password. It is not a secret: the keychain holds only the
/// development identity, and it is fixed so builds can unlock the keychain
/// without prompting.
pub const KEYCHAIN_PASSWORD: &str = "chartreuse-dev-signing";
/// The certificate's common name, as `security find-identity` lists it.
pub const COMMON_NAME: &str = "Chartreuse Dev Signing";
/// Ten years. A new certificate is a new identity, which loses the grant.
const VALIDITY_DAYS: &str = "3650";
/// Protects the temporary PKCS #12 file during the moment it exists.
const P12_PASSWORD: &str = "chartreuse";
const OPENSSL: &str = "/usr/bin/openssl";
const SECURITY: &str = "/usr/bin/security";

/// The development keychain's path under `home`.
#[must_use]
pub fn keychain_path(home: &Path) -> PathBuf {
    home.join("Library").join("Keychains").join(KEYCHAIN_FILE)
}

/// Unlocks `keychain`. A keychain starts out locked after a restart.
#[must_use]
pub fn unlock_step(keychain: &Path) -> Step {
    Step::new(
        "unlock the keychain",
        SECURITY,
        &[&"unlock-keychain", &"-p", &KEYCHAIN_PASSWORD, &keychain],
    )
}

/// The commands that create the identity in `keychain`, with the certificate and
/// key passing through files in `work`.
#[must_use]
pub fn create_steps(keychain: &Path, work: &Path) -> Vec<Step> {
    let key = work.join("key.pem");
    let certificate = work.join("certificate.pem");
    let p12 = work.join("identity.p12");
    let subject = format!("/CN={COMMON_NAME}");
    let p12_password = format!("pass:{P12_PASSWORD}");
    vec![
        Step::new(
            "create a self-signed code-signing certificate",
            OPENSSL,
            &[
                &"req",
                &"-x509",
                &"-newkey",
                &"rsa:2048",
                &"-nodes",
                &"-keyout",
                &key,
                &"-out",
                &certificate,
                &"-days",
                &VALIDITY_DAYS,
                &"-subj",
                &subject,
                &"-addext",
                &"basicConstraints=critical,CA:false",
                &"-addext",
                &"keyUsage=critical,digitalSignature",
                &"-addext",
                &"extendedKeyUsage=critical,codeSigning",
            ],
        ),
        Step::new(
            "package the certificate and key",
            OPENSSL,
            &[
                &"pkcs12",
                &"-export",
                &"-inkey",
                &key,
                &"-in",
                &certificate,
                &"-name",
                &COMMON_NAME,
                &"-out",
                &p12,
                &"-passout",
                &p12_password,
            ],
        ),
        Step::new(
            "create the keychain",
            SECURITY,
            &[&"create-keychain", &"-p", &KEYCHAIN_PASSWORD, &keychain],
        ),
        Step::new(
            "keep the keychain unlocked (no timeout, no lock on sleep)",
            SECURITY,
            &[&"set-keychain-settings", &keychain],
        ),
        unlock_step(keychain),
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
                &P12_PASSWORD,
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
                &KEYCHAIN_PASSWORD,
                &keychain,
            ],
        ),
    ]
}

/// The SHA-1 hash of the [`COMMON_NAME`] identity in `security find-identity`
/// output, which lists identities as `  1) <SHA-1> "<name>" (<status>)`.
#[must_use]
pub fn identity_hash(find_identity_output: &str) -> Option<&str> {
    let quoted = format!("\"{COMMON_NAME}\"");
    find_identity_output
        .lines()
        .filter(|line| line.contains(&quoted))
        .find_map(|line| line.split_once(')')?.1.split_whitespace().next())
}

/// Unlocks `keychain`, so `codesign` can use it without prompting, and returns
/// the SHA-1 hash of the development identity in it.
fn unlock(keychain: &Path) -> Result<String> {
    capture(&mut unlock_step(keychain).command())?;
    let output = capture(
        tool(SECURITY)
            .args(["find-identity", "-p", "codesigning"])
            .arg(keychain),
    )?;
    identity_hash(&String::from_utf8_lossy(&output.stdout))
        .map(str::to_owned)
        .ok_or_else(|| {
            Error(format!(
                "{} has no \"{COMMON_NAME}\" identity. Delete it with `security \
                 delete-keychain '{}'` and run `cargo xtask dev-cert` again.",
                keychain.display(),
                keychain.display()
            ))
        })
}

/// The development identity, unlocked for signing, if `cargo xtask dev-cert` has
/// created its keychain.
pub fn installed() -> Result<Option<Identity>> {
    let Some(home) = std::env::var_os("HOME") else {
        return Ok(None);
    };
    let keychain = keychain_path(Path::new(&home));
    if !keychain.exists() {
        return Ok(None);
    }
    let hash = unlock(&keychain)?;
    Ok(Some(Identity::DevCert { keychain, hash }))
}

pub fn dev_cert() -> Result {
    if !cfg!(target_os = "macos") {
        return Err(Error("dev-cert is only needed on macOS".into()));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| Error("HOME is not set".into()))?;
    let keychain = keychain_path(Path::new(&home));
    let bundle_id = Flavor::Development.bundle_id();
    if keychain.exists() {
        let hash = unlock(&keychain)?;
        eprintln!(
            "the development signing identity already exists: \"{COMMON_NAME}\" ({hash}) in {}\n\
             To remove it: security delete-keychain '{}'\n\
             macOS keeps one Screen Recording record per bundle id, pinned to the build \
             that first asked. After recreating the identity, no new build matches the old \
             record and macOS would ask on every launch, so clear it once:\n    \
             tccutil reset ScreenCapture {bundle_id}",
            keychain.display(),
            keychain.display()
        );
        return Ok(());
    }

    let folder = keychain.parent().expect("the keychain path has a folder");
    std::fs::create_dir_all(folder).context(|| format!("creating {}", folder.display()))?;

    let work = PrivateDir::create("chartreuse-dev-cert")?;

    for step in create_steps(&keychain, work.path()) {
        eprintln!("dev-cert: {}", step.what);
        if let Err(error) = capture(&mut step.command()) {
            if keychain.exists() {
                // Start the next attempt from scratch.
                let _ = capture(tool(SECURITY).arg("delete-keychain").arg(&keychain));
            }
            return Err(error);
        }
    }
    drop(work);

    let hash = unlock(&keychain)?;
    eprintln!(
        "\ncreated the development signing identity \"{COMMON_NAME}\" ({hash}) in {}\n\
         `cargo xtask bundle` and `cargo xtask run` sign with it whenever \
         CHARTREUSE_SIGN_IDENTITY is unset.\n\n\
         macOS keeps one Screen Recording record per bundle id, pinned to the build \
         that first asked. If any earlier build asked (an ad-hoc one, one signed with \
         another certificate, or one signed with an earlier dev identity), no build \
         signed with this identity matches that record and macOS would ask on every \
         launch. Clear it once, then launch, allow Chartreuse Dev in System Settings, \
         and relaunch:\n\n    \
         tccutil reset ScreenCapture {bundle_id}\n    \
         cargo xtask run\n\n\
         To remove the identity: security delete-keychain '{}'",
        keychain.display(),
        keychain.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keychain() -> PathBuf {
        keychain_path(Path::new("/Users/jo"))
    }

    #[test]
    fn the_keychain_lives_in_the_users_keychains_folder() {
        assert_eq!(
            keychain(),
            Path::new("/Users/jo/Library/Keychains/chartreuse-dev-signing.keychain-db")
        );
    }

    #[test]
    fn every_keychain_command_names_the_development_keychain() {
        // Without an explicit keychain, `security` falls back to the default
        // (login) keychain.
        let keychain = keychain();
        let steps = create_steps(&keychain, Path::new("/tmp/work"));
        let security: Vec<&Step> = steps.iter().filter(|s| s.program == SECURITY).collect();
        assert!(!security.is_empty());
        for step in security {
            assert!(
                step.args.contains(&keychain.as_os_str().to_owned()),
                "{step:?}"
            );
        }
        assert!(unlock_step(&keychain)
            .args
            .contains(&keychain.as_os_str().to_owned()));
    }

    #[test]
    fn commands_that_need_the_keychain_password_are_given_it() {
        // `security` asks on the terminal, or in a dialog, for any password it
        // is not given.
        let keychain = keychain();
        let mut steps = create_steps(&keychain, Path::new("/tmp/work"));
        steps.push(unlock_step(&keychain));
        for step in &steps {
            let subcommand = step.args[0].to_str().unwrap();
            let flag = match subcommand {
                "create-keychain" | "unlock-keychain" => "-p",
                "set-key-partition-list" => "-k",
                _ => continue,
            };
            let at = step.args.iter().position(|arg| arg == flag);
            assert_eq!(
                at.map(|at| step.args[at + 1].to_str().unwrap()),
                Some(KEYCHAIN_PASSWORD),
                "{step:?}"
            );
        }
    }

    #[test]
    fn key_material_stays_in_the_work_directory() {
        let work = Path::new("/tmp/work");
        for step in create_steps(&keychain(), work) {
            for arg in &step.args {
                let arg = Path::new(arg);
                if arg
                    .extension()
                    .is_some_and(|ext| ext == "pem" || ext == "p12")
                {
                    assert!(arg.starts_with(work), "{step:?}");
                }
            }
        }
    }

    #[test]
    fn identity_hash_is_read_from_find_identity_output() {
        let output = "\nPolicy: Code Signing\n  Matching identities\n  \
            1) 1111111111111111111111111111111111111111 \"Apple Development: Jo (ABC123)\"\n  \
            2) AB76B80BF7B3F1D0FC53E1242D3F79D566418749 \"Chartreuse Dev Signing\" \
            (CSSMERR_TP_NOT_TRUSTED)\n     2 identities found\n\n  Valid identities only\n  \
            1) 1111111111111111111111111111111111111111 \"Apple Development: Jo (ABC123)\"\n     \
            1 valid identities found\n";
        assert_eq!(
            identity_hash(output),
            Some("AB76B80BF7B3F1D0FC53E1242D3F79D566418749")
        );
        let empty = "\nPolicy: Code Signing\n  Matching identities\n     0 identities found\n";
        assert_eq!(identity_hash(empty), None);
    }
}
