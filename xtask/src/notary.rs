//! Notarization with `xcrun notarytool`, and stapling with `xcrun stapler`.
//!
//! Credentials come from the environment, in one of two forms:
//!
//! - [`PROFILE_ENV`]: a keychain profile stored once with `xcrun notarytool
//!   store-credentials` (the usual choice on a developer machine).
//! - An App Store Connect API key (the usual choice in CI): [`KEY_ID_ENV`],
//!   [`ISSUER_ENV`], and the `.p8` key as a file ([`KEY_PATH_ENV`]) or as its
//!   contents ([`KEY_ENV`], for CI secrets; written to a private temporary
//!   file, since `notarytool` only reads keys from files).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::util::{run, tool, Context, Error, Result, Step};

pub const PROFILE_ENV: &str = "CHARTREUSE_NOTARY_PROFILE";
pub const KEY_PATH_ENV: &str = "CHARTREUSE_NOTARY_KEY_PATH";
pub const KEY_ENV: &str = "CHARTREUSE_NOTARY_KEY";
pub const KEY_ID_ENV: &str = "CHARTREUSE_NOTARY_KEY_ID";
pub const ISSUER_ENV: &str = "CHARTREUSE_NOTARY_ISSUER";

/// How the `.p8` API key is supplied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKey {
    File(PathBuf),
    Contents(String),
}

/// What `notarytool` authenticates with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credentials {
    /// A keychain profile stored with `xcrun notarytool store-credentials`.
    Profile(String),
    /// An App Store Connect API key, its key ID, and its issuer ID.
    ApiKey {
        key: ApiKey,
        id: String,
        issuer: String,
    },
}

impl Credentials {
    /// The credentials from the environment, as read by `var`. Missing or
    /// conflicting variables are an error naming them.
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let get = |name: &str| {
            var(name)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        let profile = get(PROFILE_ENV);
        let key_path = get(KEY_PATH_ENV);
        let key = get(KEY_ENV);
        let id = get(KEY_ID_ENV);
        let issuer = get(ISSUER_ENV);
        let any_key = key_path.is_some() || key.is_some() || id.is_some() || issuer.is_some();

        if let Some(profile) = profile {
            if any_key {
                return Err(Error(format!(
                    "{PROFILE_ENV} and App Store Connect API key variables are both set; \
                     notarize with one: unset {PROFILE_ENV}, or {KEY_PATH_ENV}, {KEY_ENV}, \
                     {KEY_ID_ENV}, and {ISSUER_ENV}."
                )));
            }
            return Ok(Self::Profile(profile));
        }
        if !any_key {
            return Err(Error(format!(
                "notarization needs credentials. Set {PROFILE_ENV} to a keychain profile \
                 stored with `xcrun notarytool store-credentials`, or set {KEY_ID_ENV}, \
                 {ISSUER_ENV}, and {KEY_PATH_ENV} (the .p8 file) or {KEY_ENV} (its \
                 contents) for an App Store Connect API key."
            )));
        }
        let key = match (key_path, key) {
            (Some(_), Some(_)) => {
                return Err(Error(format!(
                    "{KEY_PATH_ENV} and {KEY_ENV} are both set; set one."
                )));
            }
            (Some(path), None) => Some(ApiKey::File(path.into())),
            (None, Some(contents)) => Some(ApiKey::Contents(contents)),
            (None, None) => None,
        };
        match (key, id, issuer) {
            (Some(key), Some(id), Some(issuer)) => Ok(Self::ApiKey { key, id, issuer }),
            (key, id, issuer) => {
                let missing: Vec<String> = [
                    key.is_none()
                        .then(|| format!("{KEY_PATH_ENV} (or {KEY_ENV})")),
                    id.is_none().then(|| KEY_ID_ENV.to_owned()),
                    issuer.is_none().then(|| ISSUER_ENV.to_owned()),
                ]
                .into_iter()
                .flatten()
                .collect();
                Err(Error(format!(
                    "the App Store Connect API key for notarization also needs {}.",
                    missing.join(", ")
                )))
            }
        }
    }

    /// Fails early, before a long build, if an API key file does not exist.
    pub fn check(&self) -> Result {
        match self {
            Self::ApiKey {
                key: ApiKey::File(path),
                ..
            } if !path.is_file() => Err(Error(format!(
                "{KEY_PATH_ENV} names {}, which is not a file",
                path.display()
            ))),
            _ => Ok(()),
        }
    }

    /// `notarytool`'s authentication arguments. An API key given by contents is
    /// first written to `dir`, which should be private.
    pub fn auth_args(&self, dir: &Path) -> Result<Vec<OsString>> {
        let (key, id, issuer) = match self {
            Self::Profile(profile) => {
                return Ok(vec!["--keychain-profile".into(), profile.into()]);
            }
            Self::ApiKey { key, id, issuer } => (key, id, issuer),
        };
        let path = match key {
            ApiKey::File(path) => path.clone(),
            ApiKey::Contents(contents) => {
                let path = dir.join(format!("AuthKey_{id}.p8"));
                std::fs::write(&path, format!("{contents}\n"))
                    .context(|| format!("writing {}", path.display()))?;
                path
            }
        };
        Ok(vec![
            "--key".into(),
            path.into(),
            "--key-id".into(),
            id.into(),
            "--issuer".into(),
            issuer.into(),
        ])
    }
}

/// The `xcrun` arguments that submit `file` (a zip, disk image, or package)
/// and wait for the verdict, printed as JSON.
#[must_use]
pub fn submit_args(file: &Path, auth: &[OsString]) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["notarytool".into(), "submit".into(), file.into()];
    args.extend(["--wait", "--output-format", "json"].map(OsString::from));
    args.extend_from_slice(auth);
    args
}

/// The `xcrun` arguments that print the notary log of submission `id`, which
/// explains a rejection.
#[must_use]
pub fn log_args(id: &str, auth: &[OsString]) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["notarytool".into(), "log".into(), id.into()];
    args.extend_from_slice(auth);
    args
}

/// Attaches the notarization ticket to `path` (an app or a disk image), so
/// Gatekeeper can check it offline.
#[must_use]
pub fn staple_step(path: &Path) -> Step {
    Step::new(
        "staple the notarization ticket",
        "xcrun",
        &[&"stapler", &"staple", &path],
    )
}

/// Checks the ticket stapled to `path`.
#[must_use]
pub fn validate_step(path: &Path) -> Step {
    Step::new(
        "validate the stapled ticket",
        "xcrun",
        &[&"stapler", &"validate", &path],
    )
}

/// The string value of `key` in a flat JSON object, such as `notarytool`'s
/// `{"id":"…","status":"Accepted","message":"…"}`. Values with escapes are
/// not needed and not supported.
#[must_use]
pub fn json_string<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let quoted = format!("\"{key}\"");
    let after_key = &json[json.find(&quoted)? + quoted.len()..];
    let value = after_key.trim_start().strip_prefix(':')?.trim_start();
    let value = value.strip_prefix('"')?;
    value.find('"').map(|end| &value[..end])
}

/// Submits `file` for notarization and waits: an error unless Apple accepts
/// it, after printing the notary log that says why.
pub fn notarize(file: &Path, auth: &[OsString]) -> Result {
    let args = submit_args(file, auth);
    eprintln!(
        "$ xcrun notarytool submit {} --wait (this takes minutes)",
        file.display()
    );
    let output = tool("xcrun")
        .args(&args)
        .stderr(Stdio::inherit())
        .output()
        .context(|| "could not run xcrun notarytool".into())?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let status = json_string(&stdout, "status");
    eprintln!("notarytool: {}", stdout.trim());
    if output.status.success() && status == Some("Accepted") {
        return Ok(());
    }
    if let Some(id) = json_string(&stdout, "id") {
        // Best effort: the error below is what matters.
        let _ = run(tool("xcrun").args(log_args(id, auth)));
    }
    Err(Error(format!(
        "notarization of {} was not accepted (status {}, {})",
        file.display(),
        status.unwrap_or("unknown"),
        output.status
    )))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn from(vars: &[(&str, &str)]) -> Result<Credentials> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Credentials::from_env(|name| vars.get(name).cloned())
    }

    #[test]
    fn no_credentials_names_every_way_to_provide_them() {
        let error = from(&[]).unwrap_err();
        for name in [PROFILE_ENV, KEY_PATH_ENV, KEY_ENV, KEY_ID_ENV, ISSUER_ENV] {
            assert!(error.0.contains(name), "{name}: {error}");
        }
        // Blank counts as unset.
        assert!(from(&[(PROFILE_ENV, "  ")]).is_err());
    }

    #[test]
    fn a_profile_or_a_complete_api_key_is_accepted() {
        assert_eq!(
            from(&[(PROFILE_ENV, "chartreuse")]).unwrap(),
            Credentials::Profile("chartreuse".into())
        );
        assert_eq!(
            from(&[
                (KEY_PATH_ENV, "/k/AuthKey_ABC.p8"),
                (KEY_ID_ENV, "ABC"),
                (ISSUER_ENV, "69a6de7e-0000"),
            ])
            .unwrap(),
            Credentials::ApiKey {
                key: ApiKey::File("/k/AuthKey_ABC.p8".into()),
                id: "ABC".into(),
                issuer: "69a6de7e-0000".into(),
            }
        );
    }

    #[test]
    fn an_incomplete_api_key_names_only_what_is_missing() {
        let error = from(&[
            (KEY_ENV, "-----BEGIN PRIVATE KEY-----"),
            (KEY_ID_ENV, "ABC"),
        ])
        .unwrap_err();
        assert!(error.0.contains(ISSUER_ENV), "{error}");
        assert!(!error.0.contains(KEY_ID_ENV), "{error}");
        assert!(!error.0.contains(KEY_PATH_ENV), "{error}");
    }

    #[test]
    fn conflicting_credentials_are_rejected() {
        let both = from(&[(PROFILE_ENV, "p"), (KEY_ID_ENV, "ABC")]).unwrap_err();
        assert!(both.0.contains(PROFILE_ENV), "{both}");
        let two_keys = from(&[
            (KEY_PATH_ENV, "/k/a.p8"),
            (KEY_ENV, "contents"),
            (KEY_ID_ENV, "ABC"),
            (ISSUER_ENV, "i"),
        ])
        .unwrap_err();
        assert!(
            two_keys.0.contains(KEY_PATH_ENV) && two_keys.0.contains(KEY_ENV),
            "{two_keys}"
        );
    }

    #[test]
    fn a_missing_key_file_fails_before_the_build() {
        let credentials = Credentials::ApiKey {
            key: ApiKey::File("/nonexistent/AuthKey_ABC.p8".into()),
            id: "ABC".into(),
            issuer: "i".into(),
        };
        let error = credentials.check().unwrap_err();
        assert!(error.0.contains(KEY_PATH_ENV), "{error}");
    }

    #[test]
    fn submission_waits_and_authenticates_with_a_profile() {
        let auth = Credentials::Profile("chartreuse".into())
            .auth_args(Path::new("/unused"))
            .unwrap();
        assert_eq!(
            submit_args(Path::new("/d/Chartreuse.dmg"), &auth),
            [
                "notarytool",
                "submit",
                "/d/Chartreuse.dmg",
                "--wait",
                "--output-format",
                "json",
                "--keychain-profile",
                "chartreuse"
            ]
            .map(OsString::from)
        );
        assert_eq!(
            log_args("2efe2717", &auth),
            [
                "notarytool",
                "log",
                "2efe2717",
                "--keychain-profile",
                "chartreuse"
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn an_api_key_given_by_contents_is_written_to_a_file() {
        let dir =
            std::env::temp_dir().join(format!("chartreuse-xtask-{}-notary", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let credentials = Credentials::ApiKey {
            key: ApiKey::Contents(
                "-----BEGIN PRIVATE KEY-----\nMIGT\n-----END PRIVATE KEY-----".into(),
            ),
            id: "ABC".into(),
            issuer: "69a6de7e-0000".into(),
        };
        let auth = credentials.auth_args(&dir).unwrap();
        let key = dir.join("AuthKey_ABC.p8");
        let written = std::fs::read_to_string(&key).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(written.starts_with("-----BEGIN PRIVATE KEY-----\n"));
        assert_eq!(
            auth,
            [
                OsString::from("--key"),
                key.into(),
                "--key-id".into(),
                "ABC".into(),
                "--issuer".into(),
                "69a6de7e-0000".into()
            ]
        );
    }

    #[test]
    fn stapling_uses_xcrun_stapler() {
        let dmg = Path::new("/d/Chartreuse.dmg");
        assert_eq!(staple_step(dmg).command().get_program(), "xcrun");
        assert_eq!(
            staple_step(dmg).args,
            ["stapler", "staple", "/d/Chartreuse.dmg"].map(OsString::from)
        );
        assert_eq!(
            validate_step(dmg).args,
            ["stapler", "validate", "/d/Chartreuse.dmg"].map(OsString::from)
        );
    }

    #[test]
    fn the_verdict_is_read_from_notarytool_json() {
        let accepted = r#"{"id":"2efe2717-52ef-43a5-96dc-0797e4ca1041","message":"Processing complete","status":"Accepted"}"#;
        assert_eq!(json_string(accepted, "status"), Some("Accepted"));
        assert_eq!(
            json_string(accepted, "id"),
            Some("2efe2717-52ef-43a5-96dc-0797e4ca1041")
        );
        let spaced = "{ \"status\" : \"Invalid\" }";
        assert_eq!(json_string(spaced, "status"), Some("Invalid"));
        assert_eq!(json_string("", "status"), None);
        assert_eq!(json_string("{\"status\": 3}", "status"), None);
    }
}
