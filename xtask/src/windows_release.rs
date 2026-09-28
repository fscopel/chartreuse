//! `cargo xtask release` on Windows: the release-flavor executable, which
//! carries the app icon as a resource, signed with `signtool`, with the
//! license and the readme in a `.zip`, and a per-user installer built with
//! Inno Setup from [`INSTALLER_SCRIPT`].
//!
//! 1. Resolve the code signing certificate from the environment
//!    ([`signing`]), so a missing or conflicting variable fails before the
//!    build: [`CERT_SHA1_ENV`] names a certificate in the current user's
//!    store (a hardware token, or a key a vendor's KSP exposes), or
//!    [`PFX_ENV`] and [`PFX_PASSWORD_ENV`] hold a base64-encoded `.pfx` and
//!    its password (CI secrets). Find Inno Setup's compiler ([`iscc`]).
//! 2. Build, stage the executable and documents, and sign the staged
//!    executable: SHA-256 file digest, RFC 3161 timestamp (SHA-256) from
//!    [`TIMESTAMP_URL_ENV`] or [`DEFAULT_TIMESTAMP_URL`]. Verify the
//!    signature with `signtool verify /pa`, then archive.
//! 3. Compile the installer from the staged files. Inno Setup signs it, and
//!    the uninstaller inside it, with the same `signtool` command (its
//!    `SignTool` directive), after which the installer is verified too.
//!
//! Inno Setup is the installer tool because it makes a per-user install, a
//! Start menu shortcut, an uninstaller, and in-place upgrades (closing the
//! running tray app) a few declarative lines, and compiles a plain-text
//! script with one executable. WiX (an MSI) needs component GUIDs and upgrade
//! tables for the same, and NSIS hand-written uninstall and registry code.
//! Neither Inno Setup nor NSIS is on GitHub's Windows Server 2025 runners any
//! more; `cargo xtask ci-install-tools` installs Inno Setup there.
//!
//! With `--allow-ad-hoc` and no certificate, nothing is signed and the names
//! end in `-unsigned`; SmartScreen then warns users on first run.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use chartreuse_core::flavor::Flavor;

use crate::icon::GENERATED_DIR;
use crate::release::{archive_dir, archive_stem, build_executable, stage, VERSION};
use crate::util::{
    decode_base64, loud_warning, run, run_redacted, tool, workspace_root, Context, Error,
    PrivateDir, Result,
};

/// The Inno Setup script, relative to the workspace root.
pub const INSTALLER_SCRIPT: &str = "packaging/windows/chartreuse.iss";
/// The name the script's `SignTool=` directive gives the sign command.
const INNO_SIGN_TOOL: &str = "chartreuse";
/// The installer's icon, in [`GENERATED_DIR`].
const INSTALLER_ICON: &str = "app-release.ico";

/// A base64-encoded `.pfx` holding the code signing certificate and its key.
pub const PFX_ENV: &str = "CHARTREUSE_WINDOWS_SIGN_PFX_BASE64";
/// The password of the `.pfx` in [`PFX_ENV`].
pub const PFX_PASSWORD_ENV: &str = "CHARTREUSE_WINDOWS_SIGN_PFX_PASSWORD";
/// The SHA-1 thumbprint of a code signing certificate in the current user's
/// personal certificate store.
pub const CERT_SHA1_ENV: &str = "CHARTREUSE_WINDOWS_SIGN_CERT_SHA1";
/// An RFC 3161 timestamp server, overriding [`DEFAULT_TIMESTAMP_URL`].
pub const TIMESTAMP_URL_ENV: &str = "CHARTREUSE_WINDOWS_SIGN_TIMESTAMP_URL";
/// DigiCert's public RFC 3161 timestamp server, which accepts signatures
/// from any certificate authority's certificates.
pub const DEFAULT_TIMESTAMP_URL: &str = "http://timestamp.digicert.com";

/// Where the code signing certificate comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Certificate {
    /// A `.pfx`, still base64-encoded, and its password.
    Pfx { base64: String, password: String },
    /// The thumbprint (40 hex digits) of a certificate in the current user's
    /// store.
    Store { sha1: String },
}

/// How the release is signed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signing {
    pub certificate: Certificate,
    pub timestamp_url: String,
}

/// The signing configuration from the environment, as read by `var` (an
/// empty value, as an unset GitHub secret is, counts as unset). `None` if no
/// certificate is set and `allow_unsigned`; otherwise a missing, incomplete,
/// or conflicting variable is an error naming it.
pub fn signing(
    var: impl Fn(&str) -> Option<String>,
    allow_unsigned: bool,
) -> Result<Option<Signing>> {
    let get = |name: &str| var(name).filter(|value| !value.trim().is_empty());
    let certificate = match (get(CERT_SHA1_ENV), get(PFX_ENV), get(PFX_PASSWORD_ENV)) {
        (Some(_), Some(_), _) => {
            return Err(Error(format!(
                "both {CERT_SHA1_ENV} and {PFX_ENV} are set: set only one of them"
            )));
        }
        (Some(sha1), None, _) => Certificate::Store {
            sha1: thumbprint(&sha1)?,
        },
        (None, Some(base64), Some(password)) => Certificate::Pfx { base64, password },
        (None, Some(_), None) => {
            return Err(Error(format!(
                "{PFX_PASSWORD_ENV} is not set: set it to the password of the .pfx in \
                 {PFX_ENV}"
            )));
        }
        (None, None, Some(_)) => {
            return Err(Error(format!(
                "{PFX_PASSWORD_ENV} is set, but {PFX_ENV} is not: set it to the \
                 base64-encoded .pfx (`base64 -w0 certificate.pfx`)"
            )));
        }
        (None, None, None) if allow_unsigned => return Ok(None),
        (None, None, None) => {
            return Err(Error(format!(
                "no Windows code signing certificate: set {CERT_SHA1_ENV} to the \
                 thumbprint of a code signing certificate in your certificate store, or \
                 {PFX_ENV} and {PFX_PASSWORD_ENV} to a base64-encoded .pfx and its \
                 password. Or pass --allow-ad-hoc to build unsigned: the names then end \
                 in -unsigned, and SmartScreen warns on first run."
            )));
        }
    };
    Ok(Some(Signing {
        certificate,
        timestamp_url: get(TIMESTAMP_URL_ENV).unwrap_or_else(|| DEFAULT_TIMESTAMP_URL.into()),
    }))
}

/// A certificate thumbprint as `signtool /sha1` takes it: 40 hex digits.
/// Spaces, and the invisible left-to-right mark that the certificate
/// dialog's copied thumbprints start with, are dropped.
fn thumbprint(value: &str) -> Result<String> {
    let sha1: String = value
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\u{200e}')
        .collect();
    if sha1.len() == 40 && sha1.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(sha1)
    } else {
        Err(Error(format!(
            "{CERT_SHA1_ENV} is not a certificate thumbprint (40 hex digits): {value:?}"
        )))
    }
}

/// The certificate as `signtool` is given it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Pfx { path: PathBuf, password: String },
    Store { sha1: String },
}

impl Key {
    /// The password to keep out of printed commands.
    fn secret(&self) -> &str {
        match self {
            Self::Pfx { password, .. } => password,
            Self::Store { .. } => "",
        }
    }
}

/// `signtool sign`'s options, before the file to sign: a SHA-256 file
/// digest, an RFC 3161 timestamp with a SHA-256 digest, and the certificate.
#[must_use]
pub fn sign_options(key: &Key, timestamp_url: &str) -> Vec<OsString> {
    let mut options: Vec<OsString> = [
        "sign",
        "/fd",
        "SHA256",
        "/tr",
        timestamp_url,
        "/td",
        "SHA256",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    match key {
        Key::Pfx { path, password } => {
            options.extend(["/f".into(), path.into(), "/p".into(), password.into()]);
        }
        Key::Store { sha1 } => options.extend(["/sha1".into(), sha1.into()]),
    }
    options
}

/// The highest Windows SDK version among the directory names in the SDK's
/// `bin` folder (`10.0.26100.0`, …), compared number by number.
#[must_use]
pub fn latest_sdk_version<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let numbers = |name: &str| {
        name.split('.')
            .map(str::parse::<u32>)
            .collect::<std::result::Result<Vec<_>, _>>()
            .ok()
            .filter(|numbers| numbers.len() == 4)
    };
    names
        .into_iter()
        .filter_map(|name| numbers(name).map(|numbers| (numbers, name)))
        .max()
        .map(|(_, name)| name)
}

/// `signtool.exe` from the newest Windows SDK for this machine's
/// architecture, or plain `signtool.exe` (from `PATH`, as in a Visual Studio
/// developer prompt) if there is none.
fn signtool() -> PathBuf {
    let program_files =
        std::env::var_os("ProgramFiles(x86)").unwrap_or_else(|| r"C:\Program Files (x86)".into());
    let bin = Path::new(&program_files)
        .join("Windows Kits")
        .join("10")
        .join("bin");
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => "x86",
    };
    let versions: Vec<String> = std::fs::read_dir(&bin)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                .filter(|name| bin.join(name).join(arch).join("signtool.exe").is_file())
                .collect()
        })
        .unwrap_or_default();
    match latest_sdk_version(versions.iter().map(String::as_str)) {
        Some(version) => bin.join(version).join(arch).join("signtool.exe"),
        None => PathBuf::from("signtool.exe"),
    }
}

/// Signs files with the release certificate. A `.pfx` is decoded into a
/// private temporary folder, removed when the signer is dropped.
#[derive(Debug)]
pub struct Signer {
    signtool: PathBuf,
    key: Key,
    timestamp_url: String,
    _work: Option<PrivateDir>,
}

impl Signer {
    pub fn new(signing: Signing) -> Result<Self> {
        let (key, work) = match signing.certificate {
            Certificate::Store { sha1 } => (Key::Store { sha1 }, None),
            Certificate::Pfx { base64, password } => {
                let work = PrivateDir::create("chartreuse-windows-signing")?;
                let path = work.path().join("certificate.pfx");
                std::fs::write(&path, decode_base64(PFX_ENV, &base64)?)
                    .context(|| format!("writing {}", path.display()))?;
                (Key::Pfx { path, password }, Some(work))
            }
        };
        Ok(Self {
            signtool: signtool(),
            key,
            timestamp_url: signing.timestamp_url,
            _work: work,
        })
    }

    /// Signs `file` in place, then verifies it.
    pub fn sign(&self, file: &Path) -> Result {
        run_redacted(
            tool(&self.signtool)
                .args(sign_options(&self.key, &self.timestamp_url))
                .arg(file),
            &[self.key.secret()],
        )?;
        self.verify(file)
    }

    /// Checks `file`'s signature against the Windows Authenticode policy: a
    /// trusted certificate chain, and an intact file.
    pub fn verify(&self, file: &Path) -> Result {
        run(tool(&self.signtool).args(["verify", "/pa"]).arg(file))
    }

    /// This signer's `signtool sign` command in Inno Setup's sign tool
    /// syntax ([`inno_sign_command`]).
    fn inno_command(&self) -> Result<String> {
        let mut command = vec![self.signtool.clone().into_os_string()];
        command.extend(sign_options(&self.key, &self.timestamp_url));
        inno_sign_command(&command)
    }
}

/// A command line in Inno Setup's sign tool syntax (`/S<name>=<command>`):
/// every part quoted with `$q`, `$` escaped as `$$`, and `$f`, the quoted
/// file to sign, at the end. A part holding a double quote, or ending in a
/// backslash (which would escape the closing quote), cannot be passed and is
/// an error.
pub fn inno_sign_command(command: &[OsString]) -> Result<String> {
    let mut line = String::new();
    for part in command {
        let Some(part) = part.to_str() else {
            return Err(Error(format!(
                "the signing command cannot hold {part:?}: it is not Unicode"
            )));
        };
        if part.contains('"') || part.ends_with('\\') {
            return Err(Error(
                "a signing argument (the certificate path or password, or the timestamp \
                 URL) holds a double quote or ends in a backslash, which Inno Setup cannot \
                 pass to signtool"
                    .into(),
            ));
        }
        line.push_str("$q");
        line.push_str(&inno_escape(part));
        line.push_str("$q ");
    }
    line.push_str("$f");
    Ok(line)
}

/// `text` as Inno Setup's sign tool syntax writes it: `$` escaped as `$$`.
fn inno_escape(text: &str) -> String {
    text.replace('$', "$$")
}

/// What to mask where ISCC's command line is printed: the `.pfx` password
/// (`""` for none), and the password as the sign command writes it
/// ([`inno_escape`]). That second form differs when the password holds a
/// `$`, and GitHub Actions masks only a secret's exact value.
fn iscc_secrets(password: &str) -> [String; 2] {
    [password.to_owned(), inno_escape(password)]
}

/// What `cargo xtask release` defines on ISCC's command line for
/// [`INSTALLER_SCRIPT`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallerDefines {
    pub version: String,
    /// The release flavor's bundle id, which names the value Open at login
    /// adds under the `Run` key (and Explorer under `StartupApproved\Run`).
    pub bundle_id: String,
    /// The staged release: the executable and [`DOCUMENTS`].
    pub source_dir: PathBuf,
    pub icon_file: PathBuf,
    pub output_dir: PathBuf,
    pub output_base_filename: String,
}

/// ISCC's arguments: progress-only output, the defines, and when
/// `sign_command` is given the `Sign` define and the sign tool
/// ([`INNO_SIGN_TOOL`]) that runs it, then `script`.
#[must_use]
pub fn iscc_args(
    defines: &InstallerDefines,
    sign_command: Option<&str>,
    script: &Path,
) -> Vec<OsString> {
    let define = |name: &str, value: &OsStr| {
        let mut arg = OsString::from(format!("/D{name}="));
        arg.push(value);
        arg
    };
    let mut args = vec![
        OsString::from("/Qp"),
        define("AppVersion", defines.version.as_ref()),
        define("BundleId", defines.bundle_id.as_ref()),
        define("SourceDir", defines.source_dir.as_os_str()),
        define("IconFile", defines.icon_file.as_os_str()),
        define("OutputDir", defines.output_dir.as_os_str()),
        define("OutputBaseFilename", defines.output_base_filename.as_ref()),
    ];
    if let Some(command) = sign_command {
        args.push("/DSign".into());
        args.push(format!("/S{INNO_SIGN_TOOL}={command}").into());
    }
    args.push(script.into());
    args
}

/// Inno Setup 6's compiler, `ISCC.exe`, where its installer puts it: for all
/// users (under Program Files) or for the current user (under
/// `%LOCALAPPDATA%\Programs`).
pub fn iscc() -> Result<PathBuf> {
    let roots = [
        ("ProgramFiles(x86)", ""),
        ("ProgramFiles", ""),
        ("LOCALAPPDATA", "Programs"),
    ];
    roots
        .into_iter()
        .filter_map(|(var, sub)| std::env::var_os(var).map(|root| Path::new(&root).join(sub)))
        .map(|root| root.join("Inno Setup 6").join("ISCC.exe"))
        .find(|iscc| iscc.is_file())
        .ok_or_else(|| {
            Error(
                "Inno Setup 6 is not installed; it builds the installer. Install it with \
                 `winget install JRSoftware.InnoSetup` (or `choco install innosetup`, which \
                 `cargo xtask ci-install-tools` runs)."
                    .into(),
            )
        })
}

/// Compiles the installer for the files in `staging` into `dist`, signed
/// (and verified) by `signer` if given, and returns its path.
fn build_installer(
    iscc: &Path,
    staging: &Path,
    dist: &Path,
    stem: &str,
    signer: Option<&Signer>,
) -> Result<PathBuf> {
    let defines = InstallerDefines {
        version: VERSION.into(),
        bundle_id: Flavor::Release.bundle_id().into(),
        source_dir: staging.into(),
        icon_file: workspace_root().join(GENERATED_DIR).join(INSTALLER_ICON),
        output_dir: dist.into(),
        output_base_filename: format!("{stem}-setup"),
    };
    let sign_command = signer.map(Signer::inno_command).transpose()?;
    let script = workspace_root().join(INSTALLER_SCRIPT);
    // The printed command holds the password as the sign command escapes it,
    // and ISCC may print the sign command, unescaped, as it runs it.
    let password = signer.map_or("", |signer| signer.key.secret());
    run_redacted(
        tool(iscc).args(iscc_args(&defines, sign_command.as_deref(), &script)),
        &iscc_secrets(password),
    )?;
    let installer = dist.join(format!("{}.exe", defines.output_base_filename));
    if let Some(signer) = signer {
        signer.verify(&installer)?;
    }
    Ok(installer)
}

pub fn release(allow_unsigned: bool, dist: &Path, extension: &str) -> Result<Vec<PathBuf>> {
    let signer = match signing(|name| std::env::var(name).ok(), allow_unsigned)? {
        Some(signing) => Some(Signer::new(signing)?),
        None => {
            loud_warning(&[
                "No Windows code signing certificate: the release is UNSIGNED.",
                "SmartScreen warns on first run. Names end in -unsigned.",
            ]);
            None
        }
    };
    let iscc = iscc()?;
    let executable = build_executable()?;
    let stem = archive_stem(VERSION, "windows", std::env::consts::ARCH, signer.is_none());
    let staging = stage(&stem, &executable)?;
    if let Some(signer) = &signer {
        let name = executable.file_name().unwrap_or_default();
        signer.sign(&staging.join(name))?;
    }
    let archive = dist.join(format!("{stem}.{extension}"));
    archive_dir(&staging, &archive)?;
    let installer = build_installer(&iscc, &staging, dist, &stem, signer.as_ref())?;
    Ok(vec![archive, installer])
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};

    use super::*;
    use crate::util::redact;

    fn from(vars: &[(&str, &str)], allow_unsigned: bool) -> Result<Option<Signing>> {
        let vars: HashMap<&str, &str> = vars.iter().copied().collect();
        signing(
            |name| vars.get(name).map(|v| (*v).to_owned()),
            allow_unsigned,
        )
    }

    const SHA1: &str = "0123456789abcdef0123456789ABCDEF01234567";

    #[test]
    fn no_certificate_is_an_error_naming_the_variables_unless_unsigned_is_allowed() {
        let error = from(&[], false).unwrap_err();
        for name in [CERT_SHA1_ENV, PFX_ENV, PFX_PASSWORD_ENV, "--allow-ad-hoc"] {
            assert!(error.0.contains(name), "{name}: {error}");
        }
        // An unset GitHub secret is an empty string.
        assert_eq!(
            from(&[(PFX_ENV, ""), (CERT_SHA1_ENV, " ")], true).unwrap(),
            None
        );
    }

    #[test]
    fn an_incomplete_or_conflicting_certificate_is_an_error_even_when_unsigned_is_allowed() {
        let cases: [(&[(&str, &str)], &str); 3] = [
            (&[(PFX_ENV, "MIIK")], PFX_PASSWORD_ENV),
            (&[(PFX_PASSWORD_ENV, "pass")], PFX_ENV),
            (
                &[
                    (PFX_ENV, "MIIK"),
                    (PFX_PASSWORD_ENV, "pass"),
                    (CERT_SHA1_ENV, SHA1),
                ],
                CERT_SHA1_ENV,
            ),
        ];
        for (vars, named) in cases {
            let error = from(vars, true).unwrap_err();
            assert!(error.0.contains(named), "{vars:?}: {error}");
        }
    }

    #[test]
    fn a_pfx_is_used_with_the_default_timestamp_server() {
        assert_eq!(
            from(&[(PFX_ENV, "MIIK"), (PFX_PASSWORD_ENV, " pass ")], false).unwrap(),
            Some(Signing {
                certificate: Certificate::Pfx {
                    base64: "MIIK".into(),
                    password: " pass ".into()
                },
                timestamp_url: DEFAULT_TIMESTAMP_URL.into(),
            })
        );
    }

    #[test]
    fn a_thumbprint_copied_from_the_certificate_dialog_is_cleaned_up() {
        let copied = "\u{200e}01 23 45 67 89 ab cd ef 01 23 45 67 89 AB CD EF 01 23 45 67";
        let signing = from(
            &[
                (CERT_SHA1_ENV, copied),
                (TIMESTAMP_URL_ENV, "http://ts.example"),
            ],
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            signing.certificate,
            Certificate::Store { sha1: SHA1.into() }
        );
        assert_eq!(signing.timestamp_url, "http://ts.example");
    }

    #[test]
    fn a_malformed_thumbprint_is_rejected() {
        for bad in [
            "0123".to_owned(),
            format!("{SHA1}0"),
            SHA1.replace('0', "g"),
        ] {
            let error = from(&[(CERT_SHA1_ENV, bad.as_str())], true).unwrap_err();
            assert!(error.0.contains(CERT_SHA1_ENV), "{bad}: {error}");
        }
    }

    #[test]
    fn every_signature_uses_sha256_and_a_timestamp() {
        let keys = [
            Key::Pfx {
                path: "C:\\t\\certificate.pfx".into(),
                password: "pass".into(),
            },
            Key::Store { sha1: SHA1.into() },
        ];
        for key in keys {
            let options = sign_options(&key, "http://ts.example");
            let options: Vec<&str> = options.iter().map(|o| o.to_str().unwrap()).collect();
            assert_eq!(options[0], "sign");
            for pair in [
                ["/fd", "SHA256"],
                ["/tr", "http://ts.example"],
                ["/td", "SHA256"],
            ] {
                assert!(
                    options.windows(2).any(|w| w == pair),
                    "{pair:?} in {options:?}"
                );
            }
            let certificate: &[&str] = match &key {
                Key::Pfx { .. } => &["/f", "C:\\t\\certificate.pfx", "/p", "pass"],
                Key::Store { .. } => &["/sha1", SHA1],
            };
            assert!(options.ends_with(certificate), "{options:?}");
        }
    }

    #[test]
    fn the_newest_sdk_is_picked_by_number_not_by_name() {
        let names = [
            "10.0.9200.0",
            "10.0.26100.0",
            "10.0.22621.0",
            "arm64",
            "x64",
        ];
        assert_eq!(latest_sdk_version(names), Some("10.0.26100.0"));
        assert_eq!(latest_sdk_version(["x64", "10.0"]), None);
    }

    #[test]
    fn the_inno_sign_command_quotes_every_part_and_escapes_dollars() {
        let command: Vec<OsString> = [
            r"C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\signtool.exe",
            "sign",
            "/p",
            "pa$$ word",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert_eq!(
            inno_sign_command(&command).unwrap(),
            concat!(
                r"$qC:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\signtool.exe$q ",
                "$qsign$q $q/p$q $qpa$$$$ word$q $f"
            )
        );
    }

    #[test]
    fn the_inno_sign_command_rejects_what_it_cannot_quote() {
        for bad in [r#"pass"word"#, r"password\"] {
            let command = [OsString::from("signtool.exe"), bad.into()];
            assert!(inno_sign_command(&command).is_err(), "{bad}");
        }
    }

    fn defines() -> InstallerDefines {
        InstallerDefines {
            version: VERSION.into(),
            bundle_id: Flavor::Release.bundle_id().into(),
            source_dir: r"C:\target\release\Chartreuse".into(),
            icon_file: r"C:\assets\app-release.ico".into(),
            output_dir: r"C:\target\dist".into(),
            output_base_filename: "Chartreuse-setup".into(),
        }
    }

    #[test]
    fn the_printed_iscc_command_hides_a_password_holding_dollars() {
        let password = "hunter$2";
        let key = Key::Pfx {
            path: r"C:\t\certificate.pfx".into(),
            password: password.into(),
        };
        let mut command = vec![OsString::from("signtool.exe")];
        command.extend(sign_options(&key, DEFAULT_TIMESTAMP_URL));
        let sign_command = inno_sign_command(&command).unwrap();
        let args = iscc_args(&defines(), Some(&sign_command), Path::new("x.iss"));
        let line: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let shown = redact(&line.join(" "), &iscc_secrets(password));
        assert!(!shown.contains("hunter"), "{shown}");
        assert!(shown.contains("$q/p$q $q***$q"), "{shown}");
    }

    /// The names `script` uses as `{#Name}` or tests with `#ifdef` and
    /// `#ifndef`.
    fn script_defines(script: &str) -> BTreeSet<&str> {
        let mut names = BTreeSet::new();
        for line in script.lines().map(str::trim) {
            if line.starts_with(';') {
                continue;
            }
            if let Some(name) = line
                .strip_prefix("#ifdef ")
                .or_else(|| line.strip_prefix("#ifndef "))
            {
                names.insert(name.trim());
            }
            let mut rest = line;
            while let Some(start) = rest.find("{#") {
                rest = &rest[start + 2..];
                names.insert(rest.split('}').next().unwrap());
            }
        }
        names
    }

    #[test]
    fn the_installer_script_gets_every_define_it_uses() {
        let script = std::fs::read_to_string(workspace_root().join(INSTALLER_SCRIPT)).unwrap();
        let defines = defines();
        let args = iscc_args(&defines, Some("$qsigntool$q sign $f"), Path::new("x.iss"));
        let args: Vec<&str> = args.iter().map(|arg| arg.to_str().unwrap()).collect();
        let passed: BTreeSet<&str> = args
            .iter()
            .filter_map(|arg| arg.strip_prefix("/D"))
            .map(|define| define.split('=').next().unwrap())
            .collect();
        assert_eq!(script_defines(&script), passed);
        assert_eq!(args.last(), Some(&"x.iss"), "the script comes last");
        let sign_tool = format!("SignTool={INNO_SIGN_TOOL}");
        assert!(
            script.lines().any(|line| line.trim() == sign_tool)
                && args.contains(&format!("/S{INNO_SIGN_TOOL}=$qsigntool$q sign $f").as_str()),
            "the script signs with the sign tool ISCC is given"
        );

        let unsigned = iscc_args(&defines, None, Path::new("x.iss"));
        assert!(
            !unsigned
                .iter()
                .any(|arg| arg == "/DSign" || arg.to_string_lossy().starts_with("/S")),
            "{unsigned:?}"
        );
    }

    #[test]
    fn the_installer_installs_every_staged_file() {
        let script = std::fs::read_to_string(workspace_root().join(INSTALLER_SCRIPT)).unwrap();
        let installed: BTreeSet<&str> = script
            .lines()
            .filter_map(|line| line.strip_prefix(r#"Source: "{#SourceDir}\"#))
            .map(|rest| rest.split('"').next().unwrap())
            .collect();
        let executable = format!("{}.exe", crate::bundle::EXECUTABLE);
        let staged: BTreeSet<&str> = std::iter::once(executable.as_str())
            .chain(crate::release::DOCUMENTS)
            .collect();
        assert_eq!(installed, staged);
    }

    /// The parameters of each entry in `script`'s `[Registry]` section, by
    /// name, without their quotes.
    fn registry_entries(script: &str) -> Vec<HashMap<&str, &str>> {
        let mut entries = Vec::new();
        let mut in_registry = false;
        for line in script.lines().map(str::trim) {
            if line.starts_with('[') {
                in_registry = line == "[Registry]";
            } else if in_registry && !line.is_empty() && !line.starts_with([';', '#']) {
                let entry = line
                    .split(';')
                    .filter_map(|parameter| parameter.split_once(':'))
                    .map(|(name, value)| (name.trim(), value.trim().trim_matches('"')))
                    .collect();
                entries.push(entry);
            }
        }
        entries
    }

    #[test]
    fn uninstalling_removes_the_open_at_login_entry() {
        let script = std::fs::read_to_string(workspace_root().join(INSTALLER_SCRIPT)).unwrap();
        let entries = registry_entries(&script);
        // Where the Windows launch-at-login backend adds, and Explorer marks,
        // a value named by the bundle id.
        for subkey in [
            r"Software\Microsoft\Windows\CurrentVersion\Run",
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
        ] {
            let entry = entries
                .iter()
                .find(|entry| entry.get("Subkey") == Some(&subkey))
                .unwrap_or_else(|| panic!("no [Registry] entry for {subkey}"));
            assert_eq!(entry["Root"], "HKCU", "{subkey}");
            assert_eq!(entry["ValueName"], "{#BundleId}", "{subkey}");
            // Installing writes no value, so it never turns Open at login on.
            assert_eq!(entry["ValueType"], "none", "{subkey}");
            assert!(
                entry["Flags"]
                    .split_whitespace()
                    .any(|flag| flag == "uninsdeletevalue"),
                "{subkey}: {entry:?}"
            );
        }
    }
}
