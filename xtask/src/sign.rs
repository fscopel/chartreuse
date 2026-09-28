//! Code signing with `codesign`.
//!
//! macOS keys privacy grants such as Screen Recording on the bundle's designated
//! requirement. With a real certificate that requirement names the bundle id and
//! the certificate, so grants survive rebuilds; with an ad-hoc signature it is the
//! binary's `cdhash`, so every rebuild looks like a new app.
//!
//! Every bundle, development or release, is signed the same way: hardened
//! runtime, the checked-in [`ENTITLEMENTS`] (empty: see its documentation), and
//! nested code signed inside-out before the bundle, never with `--deep` (which
//! Apple advises against: it applies the bundle's options to everything inside).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::bundle::EXECUTABLE;
use crate::util::{capture, loud_warning, run, tool, workspace_root, Context, Result};

/// The environment variable naming the development signing identity.
pub const DEV_IDENTITY_ENV: &str = "CHARTREUSE_SIGN_IDENTITY";

/// The entitlements file every bundle is signed with, relative to the
/// workspace root. It is an empty dictionary: Chartreuse is not sandboxed (Mac
/// App Store distribution is out of scope), ScreenCaptureKit needs no
/// entitlement outside the sandbox, and nothing needs a hardened-runtime
/// exception (JIT, unsigned executable memory, disabled library validation,
/// `DYLD_` variables). `com.apple.security.get-task-allow` must never be added:
/// notarization rejects it.
pub const ENTITLEMENTS: &str = "assets/macos/Chartreuse.entitlements";

/// Folders of a bundle's `Contents` holding nested code bundles and libraries.
const NESTED_CODE_FOLDERS: [&str; 4] = ["Frameworks", "PlugIns", "XPCServices", "Library"];
/// Folders of a bundle's `Contents` where every file is an executable.
const EXECUTABLE_FOLDERS: [&str; 2] = ["MacOS", "Helpers"];
/// Extensions of nested code: bundles (directories) and libraries (files).
const CODE_EXTENSIONS: [&str; 7] = ["app", "appex", "bundle", "framework", "xpc", "dylib", "so"];

/// What to sign with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    /// A certificate in the keychain, by name or SHA-1 hash (as listed by
    /// `security find-identity -v -p codesigning`).
    Named(String),
    /// The identity `cargo xtask dev-cert` created, by SHA-1 hash, in its own
    /// keychain (which is not on the search list).
    DevCert { keychain: PathBuf, hash: String },
    /// An ad-hoc signature: runs locally, but privacy grants do not persist.
    AdHoc,
}

impl Identity {
    /// The identity from an environment variable's value; unset or blank means
    /// ad-hoc.
    #[must_use]
    pub fn from_env_value(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some(name) if !name.is_empty() => Self::Named(name.to_owned()),
            _ => Self::AdHoc,
        }
    }

    /// The development identity: the one named by [`DEV_IDENTITY_ENV`]'s value if
    /// set, else the `cargo xtask dev-cert` identity if `dev_cert` finds one, else
    /// ad-hoc.
    pub fn development(
        env_value: Option<&str>,
        dev_cert: impl FnOnce() -> Result<Option<Self>>,
    ) -> Result<Self> {
        match Self::from_env_value(env_value) {
            Self::AdHoc => Ok(dev_cert()?.unwrap_or(Self::AdHoc)),
            named => Ok(named),
        }
    }
}

/// Options that differ between development and release signing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignOptions {
    /// Request a secure timestamp from Apple (release only; needs the network).
    pub timestamp: bool,
}

/// The `codesign` arguments that sign `code`: hardened runtime always, for
/// parity between development and release. Entitlements apply to the bundle
/// alone, not to the libraries and helpers nested in it.
#[must_use]
pub fn codesign_args(
    code: &Path,
    identity: &Identity,
    options: SignOptions,
    entitlements: Option<&Path>,
) -> Vec<OsString> {
    let timestamp = if options.timestamp {
        "--timestamp"
    } else {
        "--timestamp=none"
    };
    let mut args: Vec<OsString> = ["--force", "--options", "runtime", timestamp]
        .map(OsString::from)
        .into();
    if let Some(entitlements) = entitlements {
        args.extend(["--entitlements".into(), entitlements.into()]);
    }
    match identity {
        Identity::Named(name) => args.extend(["--sign".into(), name.into()]),
        Identity::DevCert { keychain, hash } => args.extend([
            "--keychain".into(),
            keychain.into(),
            "--sign".into(),
            hash.into(),
        ]),
        Identity::AdHoc => args.extend(["--sign".into(), "-".into()]),
    }
    args.push(code.into());
    args
}

/// The code nested in `bundle` (helpers, frameworks, plug-ins, libraries, but
/// not the main executable, which signing the bundle covers), in the order it
/// must be signed: everything after the code it contains.
pub fn nested_code(bundle: &Path, main_executable: &str) -> Result<Vec<PathBuf>> {
    let contents = bundle.join("Contents");
    let mut code = Vec::new();
    for folder in EXECUTABLE_FOLDERS {
        for (path, is_dir) in entries(&contents.join(folder))? {
            if is_dir {
                visit(path, true, &mut code)?;
            } else if folder != "MacOS" || path.file_name() != Some(main_executable.as_ref()) {
                code.push(path);
            }
        }
    }
    for folder in NESTED_CODE_FOLDERS {
        find_code(&contents.join(folder), &mut code)?;
    }
    Ok(code)
}

/// Collects the code bundles and libraries below `dir`.
fn find_code(dir: &Path, code: &mut Vec<PathBuf>) -> Result {
    for (path, is_dir) in entries(dir)? {
        visit(path, is_dir, code)?;
    }
    Ok(())
}

/// Collects the code below `path`, then `path` itself if it is a code bundle
/// or library: code nested in other code comes first.
fn visit(path: PathBuf, is_dir: bool, code: &mut Vec<PathBuf>) -> Result {
    let is_code = path
        .extension()
        .is_some_and(|ext| CODE_EXTENSIONS.iter().any(|code| ext == *code));
    if is_dir {
        find_code(&path, code)?;
    }
    if is_code {
        code.push(path);
    }
    Ok(())
}

/// The entries of `dir`, sorted, with whether each is a directory (symbolic
/// links, as in a framework's `Versions/Current`, are neither followed nor
/// signed); none if it does not exist.
fn entries(dir: &Path) -> Result<Vec<(PathBuf, bool)>> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    let mut entries = Vec::new();
    for entry in read {
        let entry = entry.context(|| format!("reading {}", dir.display()))?;
        let kind = entry
            .file_type()
            .context(|| format!("reading {}", entry.path().display()))?;
        if !kind.is_symlink() {
            entries.push((entry.path(), kind.is_dir()));
        }
    }
    entries.sort();
    Ok(entries)
}

/// Extracts the designated requirement from `codesign --display -r-` output.
/// Implicit requirements (the usual case) are printed as a `# designated => …`
/// comment.
#[must_use]
pub fn designated_requirement(display_output: &str) -> Option<&str> {
    display_output
        .lines()
        .map(|line| line.trim_start_matches(['#', ' ', '\t']))
        .find_map(|line| line.strip_prefix("designated =>"))
        .map(str::trim)
}

/// True if the requirement pins the exact binary, as ad-hoc signatures do.
#[must_use]
pub fn requirement_is_ad_hoc(requirement: &str) -> bool {
    requirement.contains("cdhash")
}

/// Signs the code nested in `bundle`, then `bundle` with [`ENTITLEMENTS`],
/// then prints its designated requirement and warns if it will not keep
/// privacy grants across rebuilds.
pub fn sign(bundle: &Path, identity: &Identity, options: SignOptions, env_var: &str) -> Result {
    match identity {
        Identity::AdHoc if env_var == DEV_IDENTITY_ENV => loud_warning(&[
            &format!("{env_var} is not set and `cargo xtask dev-cert` has not been run:"),
            "signing ad-hoc. Ad-hoc signatures change with every build, and macOS ties",
            "Screen Recording to the signature, so ad-hoc builds never keep the",
            "permission and the app does not ask for it. Run `cargo xtask dev-cert`",
            &format!("once, or set {env_var} to an identity from"),
            "`security find-identity -v -p codesigning` (see README.md).",
        ]),
        Identity::AdHoc => loud_warning(&[
            &format!("{env_var} is not set: signing ad-hoc."),
            "Ad-hoc signatures change with every build, so macOS forgets the Screen",
            "Recording permission after each rebuild and captures come back blank.",
            &format!("Set {env_var} to a code-signing identity from"),
            "`security find-identity -v -p codesigning` (see README.md).",
        ]),
        Identity::DevCert { .. } => {
            eprintln!("signing with the `cargo xtask dev-cert` identity");
        }
        Identity::Named(_) => {}
    }
    for code in nested_code(bundle, EXECUTABLE)? {
        run(tool("codesign").args(codesign_args(&code, identity, options, None)))?;
    }
    let entitlements = workspace_root().join(ENTITLEMENTS);
    run(tool("codesign").args(codesign_args(
        bundle,
        identity,
        options,
        Some(&entitlements),
    )))?;

    let output = capture(tool("codesign").args(["--display", "-r-"]).arg(bundle))?;
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    match designated_requirement(&text) {
        Some(requirement) => {
            eprintln!("designated requirement: {requirement}");
            if requirement_is_ad_hoc(requirement) {
                loud_warning(&[
                    "The designated requirement contains a cdhash: Screen Recording",
                    "grants will not survive the next rebuild.",
                ]);
            }
        }
        None => eprintln!("warning: codesign printed no designated requirement:\n{text}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_or_missing_identity_means_ad_hoc() {
        assert_eq!(Identity::from_env_value(None), Identity::AdHoc);
        assert_eq!(Identity::from_env_value(Some("  ")), Identity::AdHoc);
        assert_eq!(
            Identity::from_env_value(Some(" Apple Development: Jo (ABC123) ")),
            Identity::Named("Apple Development: Jo (ABC123)".into())
        );
    }

    #[test]
    fn development_prefers_the_env_identity_then_the_dev_cert() {
        let dev_cert = || Identity::DevCert {
            keychain: "/k/dev.keychain-db".into(),
            hash: "AB76".into(),
        };
        // An explicit identity wins; the dev keychain is not even unlocked.
        let named = Identity::development(Some("Apple Development: Jo"), || {
            panic!("the dev cert must not be looked up")
        });
        assert_eq!(
            named.unwrap(),
            Identity::Named("Apple Development: Jo".into())
        );
        assert_eq!(
            Identity::development(Some(" "), || Ok(Some(dev_cert()))).unwrap(),
            dev_cert()
        );
        assert_eq!(
            Identity::development(None, || Ok(None)).unwrap(),
            Identity::AdHoc
        );
        // A dev keychain that exists but cannot be used fails the build rather
        // than silently signing ad-hoc.
        assert!(Identity::development(None, || Err("broken".into())).is_err());
    }

    #[test]
    fn signing_always_uses_the_hardened_runtime() {
        let dylib = Path::new("/tmp/Chartreuse Dev.app/Contents/Frameworks/libx.dylib");
        let args = codesign_args(
            dylib,
            &Identity::AdHoc,
            SignOptions { timestamp: false },
            None,
        );
        assert_eq!(
            args,
            [
                "--force",
                "--options",
                "runtime",
                "--timestamp=none",
                "--sign",
                "-",
                "/tmp/Chartreuse Dev.app/Contents/Frameworks/libx.dylib"
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn release_signing_is_timestamped_with_entitlements_and_not_deep() {
        let args = codesign_args(
            Path::new("/tmp/Chartreuse.app"),
            &Identity::Named("Developer ID Application: Jo (ABC123)".into()),
            SignOptions { timestamp: true },
            Some(Path::new("/src/assets/macos/Chartreuse.entitlements")),
        );
        assert_eq!(
            args,
            [
                "--force",
                "--options",
                "runtime",
                "--timestamp",
                "--entitlements",
                "/src/assets/macos/Chartreuse.entitlements",
                "--sign",
                "Developer ID Application: Jo (ABC123)",
                "/tmp/Chartreuse.app"
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn checked_in_entitlements_are_notarizable() {
        let entitlements = plist::Value::from_file(workspace_root().join(ENTITLEMENTS))
            .expect("a valid plist")
            .into_dictionary()
            .expect("a dictionary");
        // Notarization rejects get-task-allow; the sandbox would break capture
        // and is only for the Mac App Store.
        for forbidden in [
            "com.apple.security.get-task-allow",
            "com.apple.security.app-sandbox",
        ] {
            assert!(!entitlements.contains_key(forbidden), "{forbidden}");
        }
    }

    #[test]
    fn nested_code_is_found_inside_out_without_the_main_executable() {
        let app = std::env::temp_dir()
            .join(format!("chartreuse-xtask-{}-nested", std::process::id()))
            .join("Chartreuse.app");
        let _ = std::fs::remove_dir_all(&app);
        let contents = app.join("Contents");
        let framework = contents.join("Frameworks/Sparkle.framework");
        let login_item = contents.join("Library/LoginItems/Launcher.app");
        let inner = login_item.join("Contents/Frameworks/libinner.dylib");
        for file in [
            contents.join("MacOS/chartreuse"),
            contents.join("MacOS/helper"),
            contents.join("Resources/AppIcon.icns"),
            framework.join("Versions/A/Sparkle"),
            login_item.join("Contents/MacOS/Launcher"),
            inner.clone(),
        ] {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, b"").unwrap();
        }

        let code = nested_code(&app, "chartreuse").unwrap();
        std::fs::remove_dir_all(app.parent().unwrap()).unwrap();

        let position = |path: &Path| code.iter().position(|code| code == path);
        assert_eq!(code.len(), 4, "{code:?}");
        assert!(position(&contents.join("MacOS/helper")).is_some());
        assert!(position(&framework).is_some());
        // The library inside the login item is signed before the login item.
        assert!(position(&inner).unwrap() < position(&login_item).unwrap());
    }

    #[test]
    fn dev_cert_signing_searches_only_its_keychain() {
        // The dev keychain is not on the search list, so codesign must be
        // pointed at it.
        let identity = Identity::DevCert {
            keychain: "/Users/jo/Library/Keychains/dev.keychain-db".into(),
            hash: "AB76B80BF7B3F1D0FC53E1242D3F79D566418749".into(),
        };
        let args = codesign_args(
            Path::new("/tmp/Chartreuse Dev.app"),
            &identity,
            SignOptions { timestamp: false },
            None,
        );
        assert_eq!(
            &args[4..],
            [
                "--keychain",
                "/Users/jo/Library/Keychains/dev.keychain-db",
                "--sign",
                "AB76B80BF7B3F1D0FC53E1242D3F79D566418749",
                "/tmp/Chartreuse Dev.app"
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn requirement_is_parsed_from_codesign_output() {
        let certificate = "Executable=/x/Chartreuse Dev.app/Contents/MacOS/chartreuse\n\
            designated => identifier \"io.jennings.chartreuse.dev\" and anchor apple generic \
            and certificate leaf[subject.CN] = \"Apple Development: Jo (ABC123)\"\n";
        let requirement = designated_requirement(certificate).unwrap();
        assert!(requirement.starts_with("identifier \"io.jennings.chartreuse.dev\""));
        assert!(!requirement_is_ad_hoc(requirement));

        let ad_hoc = "Executable=/x/chartreuse\n# designated => cdhash H\"0123abcd\"\n";
        assert_eq!(designated_requirement(ad_hoc), Some("cdhash H\"0123abcd\""));
        assert!(requirement_is_ad_hoc(
            designated_requirement(ad_hoc).unwrap()
        ));

        assert_eq!(designated_requirement("Executable=/x/chartreuse\n"), None);
    }
}
