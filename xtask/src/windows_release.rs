//! `cargo xtask release` on Windows: the release-flavor executable, which
//! carries the app icon as a resource, signed with `signtool`, with the
//! license and the readme in a `.zip`.
//!
//! 1. Resolve the code signing certificate from the environment
//!    ([`signing`]), so a missing or conflicting variable fails before the
//!    build: [`CERT_SHA1_ENV`] names a certificate in the current user's
//!    store (a hardware token, or a key a vendor's KSP exposes), or
//!    [`PFX_ENV`] and [`PFX_PASSWORD_ENV`] hold a base64-encoded `.pfx` and
//!    its password (CI secrets).
//! 2. Build, stage the executable and documents, and sign the staged
//!    executable: SHA-256 file digest, RFC 3161 timestamp (SHA-256) from
//!    [`TIMESTAMP_URL_ENV`] or [`DEFAULT_TIMESTAMP_URL`].
//! 3. Verify the signature with `signtool verify /pa`, then archive.
//!
//! With `--allow-ad-hoc` and no certificate, nothing is signed and the names
//! end in `-unsigned`; SmartScreen then warns users on first run.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::release::{archive_dir, archive_stem, build_executable, stage, VERSION};
use crate::util::{
    decode_base64, loud_warning, run, run_redacted, tool, Context, Error, PrivateDir, Result,
};

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
    let executable = build_executable()?;
    let stem = archive_stem(VERSION, "windows", std::env::consts::ARCH, signer.is_none());
    let staging = stage(&stem, &executable)?;
    if let Some(signer) = &signer {
        let name = executable.file_name().unwrap_or_default();
        signer.sign(&staging.join(name))?;
    }
    let archive = dist.join(format!("{stem}.{extension}"));
    archive_dir(&staging, &archive)?;
    Ok(vec![archive])
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

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
}
