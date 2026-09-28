//! `cargo xtask release` on macOS: a universal (Apple silicon and Intel),
//! release-flavor `Chartreuse.app`, signed with the Developer ID identity and
//! notarized, in a signed and notarized disk image.
//!
//! 1. Resolve the signing identity and the notarization credentials
//!    ([`crate::notary`]), so a missing variable fails before the build.
//! 2. Build `chartreuse` for every [`TARGETS`] triple (installing missing
//!    targets with `rustup`) and merge the builds with `lipo`.
//! 3. Assemble `Chartreuse.app` around the universal executable and sign it
//!    ([`crate::sign`]: hardened runtime, secure timestamp, entitlements,
//!    nested code inside-out).
//! 4. Notarize the app (zipped) and staple its ticket, so the copy users drag
//!    out of the disk image carries a ticket of its own and Gatekeeper can
//!    check it offline. (Notarizing only the disk image covers the app, but
//!    staples the ticket to the image alone.)
//! 5. Image the app beside an `/Applications` link with `hdiutil`, then sign,
//!    notarize, and staple the disk image.
//! 6. Verify: `codesign --verify --strict`, Gatekeeper (`spctl`), and the
//!    stapled tickets (`stapler validate`), for the app and the disk image.
//!
//! With `--allow-ad-hoc` and no identity, the app is signed ad-hoc, nothing is
//! notarized, the disk image is left unsigned (an ad-hoc signature on a disk
//! image vouches for nothing), and its name ends in `-unsigned`; only the
//! checks that can pass without Apple's involvement run. The disk image is
//! built either way, so the release workflow and a developer without a
//! certificate exercise the same packaging. `--allow-ad-hoc` never skips
//! notarization of a build signed with a Developer ID.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use chartreuse_core::flavor::Flavor;

use crate::bundle::{self, Bundle, Profile};
use crate::dmg;
use crate::notary::{self, Credentials};
use crate::release::{archive_stem, VERSION};
use crate::sign::{Identity, SignOptions};
use crate::util::{capture, run, target_dir, tool, Context, Error, PrivateDir, Result, Step};

/// The environment variable naming the release (Developer ID) signing identity.
pub const RELEASE_IDENTITY_ENV: &str = "CHARTREUSE_RELEASE_SIGN_IDENTITY";

/// The targets merged into the universal executable.
pub const TARGETS: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];

/// The architectures of [`TARGETS`], as `lipo` names them.
const ARCHITECTURES: [&str; 2] = ["arm64", "x86_64"];

/// The architecture in release archive names.
const UNIVERSAL: &str = "universal";

/// The universal build's folder under the target directory, in the
/// `<triple>/<profile>` shape of the per-target builds.
const UNIVERSAL_DIR: &str = "universal-apple-darwin";

/// The release signing identity. A missing identity is an error naming the
/// variable, unless an ad-hoc build was explicitly requested.
pub fn release_identity(value: Option<&str>, allow_ad_hoc: bool) -> Result<Identity> {
    match Identity::from_env_value(value) {
        Identity::AdHoc if !allow_ad_hoc => Err(Error(format!(
            "{RELEASE_IDENTITY_ENV} is not set. Set it to your Developer ID Application \
             identity (see `security find-identity -v -p codesigning`), or pass \
             --allow-ad-hoc to sign ad-hoc: the disk image is then unsigned and named \
             -unsigned, and Gatekeeper blocks the app until it is allowed in System \
             Settings."
        ))),
        identity => Ok(identity),
    }
}

/// The [`TARGETS`] missing from `rustup target list --installed` output.
#[must_use]
pub fn missing_targets(installed: &str) -> Vec<&'static str> {
    TARGETS
        .into_iter()
        .filter(|target| !installed.lines().any(|line| line.trim() == *target))
        .collect()
}

/// The architectures of [`TARGETS`] missing from `lipo -archs` output.
#[must_use]
pub fn missing_architectures(archs: &str) -> Vec<&'static str> {
    ARCHITECTURES
        .into_iter()
        .filter(|arch| !archs.split_whitespace().any(|found| found == *arch))
        .collect()
}

/// The `lipo` arguments that merge `inputs` into the universal `output`.
#[must_use]
pub fn lipo_create_args(output: &Path, inputs: &[PathBuf]) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["-create".into(), "-output".into(), output.into()];
    args.extend(inputs.iter().map(OsString::from));
    args
}

/// The `codesign` arguments that sign the disk image `dmg`. The hardened
/// runtime and entitlements apply to executables, not disk images.
#[must_use]
pub fn disk_image_codesign_args(dmg: &Path, identity: &str) -> Vec<OsString> {
    vec![
        "--force".into(),
        "--timestamp".into(),
        "--sign".into(),
        identity.into(),
        dmg.into(),
    ]
}

/// The checks run on the finished app and disk image. Builds that were not
/// notarized (ad-hoc) only get the checks that can pass without Apple.
#[must_use]
pub fn verification_steps(app: &Path, dmg: &Path, notarized: bool) -> Vec<Step> {
    let mut steps = vec![Step::new(
        "verify the app's signature, nested code included",
        "codesign",
        &[&"--verify", &"--strict", &"--deep", &"--verbose=2", &app],
    )];
    if notarized {
        steps.extend([
            Step::new(
                "check that Gatekeeper accepts the app",
                "spctl",
                &[&"-a", &"-t", &"exec", &"-vv", &app],
            ),
            Step::new(
                "verify the disk image's signature",
                "codesign",
                &[&"--verify", &"--strict", &"--verbose=2", &dmg],
            ),
            Step::new(
                "check that Gatekeeper accepts the disk image",
                "spctl",
                &[
                    &"-a",
                    &"-t",
                    &"open",
                    &"--context",
                    &"context:primary-signature",
                    &"-vv",
                    &dmg,
                ],
            ),
            notary::validate_step(app),
            notary::validate_step(dmg),
        ]);
    }
    steps.push(Step::new(
        "verify the disk image's checksum",
        "hdiutil",
        &[&"verify", &dmg],
    ));
    steps
}

/// The notarization credentials, required for a build signed with a
/// Developer ID.
fn notary_credentials() -> Result<Credentials> {
    let credentials = Credentials::from_env(|name| std::env::var(name).ok()).map_err(|error| {
        Error(format!(
            "{error}\n{RELEASE_IDENTITY_ENV} is set, so the release is notarized \
             (--allow-ad-hoc only applies while it is unset)."
        ))
    })?;
    credentials.check()?;
    Ok(credentials)
}

/// Zips `app` with `ditto`, which keeps the code signature, for notarization.
fn zip_app(app: &Path, zip: &Path) -> Result {
    run(tool("ditto")
        .args(["-c", "-k", "--sequesterRsrc", "--keepParent"])
        .arg(app)
        .arg(zip))
}

/// Notarizes `file` (submitted as `submission`: the file itself, or a zip of
/// an app), then staples the ticket to `file`.
fn notarize_and_staple(file: &Path, submission: &Path, auth: &[OsString]) -> Result {
    notary::notarize(submission, auth)?;
    run(&mut notary::staple_step(file).command())
}

/// Installs the [`TARGETS`] that the pinned toolchain lacks.
fn ensure_targets() -> Result {
    let installed =
        capture(tool("rustup").args(["target", "list", "--installed"])).map_err(|error| {
            Error(format!(
                "{error}\nThe universal build needs rustup, or the targets {} installed \
                 some other way.",
                TARGETS.join(" and ")
            ))
        })?;
    let missing = missing_targets(&String::from_utf8_lossy(&installed.stdout));
    if missing.is_empty() {
        return Ok(());
    }
    run(tool("rustup").args(["target", "add"]).args(missing))
}

/// Builds every target and merges them into `target/universal-apple-darwin/release/chartreuse`,
/// returning its folder.
fn build_universal() -> Result<PathBuf> {
    ensure_targets()?;
    let profile = Profile::Release;
    let mut inputs = Vec::new();
    for target in TARGETS {
        // MACOSX_DEPLOYMENT_TARGET stays at the toolchain defaults, as for
        // development builds: LSMinimumSystemVersion already keeps the app off
        // older systems, and setting the variable here also applies it to the
        // host's proc-macro dylibs, which rustc 1.96 then fails to load.
        run(bundle::build_command(profile, Flavor::Release).args(["--target", target]))?;
        inputs.push(
            target_dir()
                .join(target)
                .join(profile.dir_name())
                .join(bundle::EXECUTABLE),
        );
    }
    let dir = target_dir().join(UNIVERSAL_DIR).join(profile.dir_name());
    std::fs::create_dir_all(&dir).context(|| format!("creating {}", dir.display()))?;
    run(tool("lipo").args(lipo_create_args(&dir.join(bundle::EXECUTABLE), &inputs)))?;
    Ok(dir)
}

/// Fails unless `executable` holds every architecture.
fn check_universal(executable: &Path) -> Result {
    let output = capture(tool("lipo").arg("-archs").arg(executable))?;
    let archs = String::from_utf8_lossy(&output.stdout);
    let missing = missing_architectures(&archs);
    if missing.is_empty() {
        eprintln!("architectures: {}", archs.trim());
        Ok(())
    } else {
        Err(Error(format!(
            "{} lacks {}",
            executable.display(),
            missing.join(" and ")
        )))
    }
}

/// Builds the disk image into `dist`, returning its path.
pub fn release(allow_ad_hoc: bool, dist: &Path, extension: &str) -> Result<PathBuf> {
    let identity = release_identity(
        std::env::var(RELEASE_IDENTITY_ENV).ok().as_deref(),
        allow_ad_hoc,
    )?;
    let signing = match &identity {
        Identity::Named(name) => Some((name.clone(), notary_credentials()?)),
        _ => None,
    };

    let dir = build_universal()?;
    let layout = Bundle {
        flavor: Flavor::Release,
        profile: Profile::Release,
        sign_options: SignOptions {
            timestamp: signing.is_some(),
        },
        identity,
        identity_env: RELEASE_IDENTITY_ENV,
    }
    .package(&dir)?;
    check_universal(&layout.executable)?;

    // Holds an API key written from the environment, and the zipped app.
    let work = PrivateDir::create("chartreuse-release")?;
    let auth = match &signing {
        Some((_, credentials)) => {
            let auth = credentials.auth_args(work.path())?;
            let zip = work.path().join("Chartreuse.zip");
            zip_app(&layout.app, &zip)?;
            notarize_and_staple(&layout.app, &zip, &auth)?;
            Some(auth)
        }
        None => None,
    };

    let stem = archive_stem(VERSION, "macos", UNIVERSAL, signing.is_none());
    let image = dist.join(format!("{stem}.{extension}"));
    dmg::create(
        &layout.app,
        Flavor::Release.display_name(),
        &dir.join("dmg"),
        &image,
    )?;
    if let (Some((name, _)), Some(auth)) = (&signing, &auth) {
        run(tool("codesign").args(disk_image_codesign_args(&image, name)))?;
        notarize_and_staple(&image, &image, auth)?;
    }
    drop(work);

    for step in verification_steps(&layout.app, &image, signing.is_some()) {
        eprintln!("release: {}", step.what);
        run(&mut step.command())?;
    }
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_release_identity_names_the_variable() {
        let error = release_identity(None, false).unwrap_err();
        assert!(error.0.contains(RELEASE_IDENTITY_ENV), "{error}");
        assert!(release_identity(Some(""), false).is_err());
    }

    #[test]
    fn ad_hoc_release_requires_opting_in() {
        assert_eq!(release_identity(None, true).unwrap(), Identity::AdHoc);
        assert_eq!(
            release_identity(Some("Developer ID Application: Jo (ABC123)"), false).unwrap(),
            Identity::Named("Developer ID Application: Jo (ABC123)".into())
        );
    }

    #[test]
    fn only_uninstalled_targets_are_added() {
        let installed = "aarch64-apple-darwin\nx86_64-pc-windows-msvc\n";
        assert_eq!(missing_targets(installed), ["x86_64-apple-darwin"]);
        let both = "aarch64-apple-darwin\nx86_64-apple-darwin\n";
        assert_eq!(missing_targets(both), Vec::<&str>::new());
        // A prefix of another target's name is not that target.
        assert_eq!(
            missing_targets("x86_64-apple-darwin-extra\naarch64-apple-darwin\n"),
            ["x86_64-apple-darwin"]
        );
    }

    #[test]
    fn a_thin_executable_is_not_universal() {
        assert_eq!(missing_architectures("x86_64 arm64\n"), Vec::<&str>::new());
        assert_eq!(missing_architectures("arm64\n"), ["x86_64"]);
        assert_eq!(missing_architectures("arm64e x86_64\n"), ["arm64"]);
    }

    #[test]
    fn lipo_merges_every_target_build() {
        let args = lipo_create_args(
            Path::new("/t/universal-apple-darwin/release/chartreuse"),
            &[
                "/t/aarch64-apple-darwin/release/chartreuse".into(),
                "/t/x86_64-apple-darwin/release/chartreuse".into(),
            ],
        );
        assert_eq!(
            args,
            [
                "-create",
                "-output",
                "/t/universal-apple-darwin/release/chartreuse",
                "/t/aarch64-apple-darwin/release/chartreuse",
                "/t/x86_64-apple-darwin/release/chartreuse"
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn the_disk_image_is_signed_with_a_timestamp() {
        let args = disk_image_codesign_args(
            Path::new("/d/Chartreuse-0.2.0-macos-universal.dmg"),
            "Developer ID Application: Jo (ABC123)",
        );
        assert_eq!(
            args,
            [
                "--force",
                "--timestamp",
                "--sign",
                "Developer ID Application: Jo (ABC123)",
                "/d/Chartreuse-0.2.0-macos-universal.dmg"
            ]
            .map(OsString::from)
        );
    }

    fn commands(steps: &[Step]) -> Vec<String> {
        steps
            .iter()
            .map(|step| {
                let args: Vec<_> = step.args.iter().map(|a| a.to_string_lossy()).collect();
                format!("{} {}", step.program, args.join(" "))
            })
            .collect()
    }

    #[test]
    fn notarized_builds_are_checked_by_gatekeeper_and_stapler() {
        let steps = verification_steps(Path::new("/t/Chartreuse.app"), Path::new("/d/C.dmg"), true);
        assert_eq!(
            commands(&steps),
            [
                "codesign --verify --strict --deep --verbose=2 /t/Chartreuse.app",
                "spctl -a -t exec -vv /t/Chartreuse.app",
                "codesign --verify --strict --verbose=2 /d/C.dmg",
                "spctl -a -t open --context context:primary-signature -vv /d/C.dmg",
                "xcrun stapler validate /t/Chartreuse.app",
                "xcrun stapler validate /d/C.dmg",
                "hdiutil verify /d/C.dmg",
            ]
        );
    }

    #[test]
    fn ad_hoc_builds_skip_the_checks_only_apple_can_pass() {
        // Gatekeeper rejects ad-hoc code, and nothing is stapled.
        let steps =
            verification_steps(Path::new("/t/Chartreuse.app"), Path::new("/d/C.dmg"), false);
        assert_eq!(
            commands(&steps),
            [
                "codesign --verify --strict --deep --verbose=2 /t/Chartreuse.app",
                "hdiutil verify /d/C.dmg",
            ]
        );
    }
}
