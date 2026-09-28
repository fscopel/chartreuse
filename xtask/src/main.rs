//! Build automation for Chartreuse, run as `cargo xtask <command>`.
//!
//! Every CI step and every developer workflow beyond `cargo build` is a command
//! here, so anything CI does can be reproduced locally.

mod bundle;
mod check;
mod ci_keychain;
mod dev_cert;
mod dmg;
mod icon;
mod info_plist;
mod launch;
mod macos_release;
mod notary;
mod release;
mod sign;
mod upload_release;
mod util;

use std::process::ExitCode;

const USAGE: &str = "\
Usage: cargo xtask <command>

Commands:
  check     cargo fmt --check, cargo clippy (warnings denied), cargo test
  bundle    build and sign target/debug/Chartreuse Dev.app (macOS); signs with
            $CHARTREUSE_SIGN_IDENTITY, else the dev-cert identity, else ad-hoc
            with a warning
  run       bundle, then launch the app with `open`, its output on this terminal
            --fake  use the synthetic platform backend: no real capture, and no
                    macOS privacy prompts (use this for automated work)
  dev-cert  create a self-signed development signing identity in its own
            keychain (macOS, once per machine), so the Screen Recording
            permission survives rebuilds
  icons     re-render the status item and tray icons checked in under
            assets/icon/generated from their SVG sources (after editing one)
  release   release build for this OS, archived into target/dist (emptied
            first) as Chartreuse-<version>-<os>-<arch>: on macOS a disk image
            (.dmg) of a universal, release-flavor Chartreuse.app, the app and
            the image signed with $CHARTREUSE_RELEASE_SIGN_IDENTITY, notarized
            ($CHARTREUSE_NOTARY_PROFILE, or $CHARTREUSE_NOTARY_KEY_ID,
            $CHARTREUSE_NOTARY_ISSUER, and $CHARTREUSE_NOTARY_KEY_PATH or
            $CHARTREUSE_NOTARY_KEY), stapled, and verified; on Windows (.zip)
            and Linux (.tar.gz) the executable with LICENSE, README.md, and
            the app icon (chartreuse.ico; icons/hicolor/<size>x<size>/apps/*.png)
            --allow-ad-hoc  macOS: when the identity is unset, sign the app
                            ad-hoc, notarize nothing, and leave the disk image
                            unsigned; its name ends in -unsigned
  ci-keychain
            CI (macOS): import the Developer ID identity from
            $CHARTREUSE_SIGN_P12_BASE64 (a base64 .p12) and
            $CHARTREUSE_SIGN_P12_PASSWORD into a temporary keychain that
            codesign uses without prompting, and print it
            --skip-if-unset  do nothing if $CHARTREUSE_SIGN_P12_BASE64 is unset
  upload-release <tag>
            attach every file in target/dist to the GitHub release <tag>
            (v<version>, matching Cargo.toml) with the GitHub CLI, replacing
            assets of the same name; needs GH_TOKEN and, outside a git
            checkout, GH_REPO=owner/repo
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["check"] => check::check(),
        ["bundle"] => bundle::Bundle::development().and_then(|b| b.build().map(drop)),
        ["run"] => launch::launch(false),
        ["run", "--fake"] => launch::launch(true),
        ["icons"] => icon::icons(),
        ["dev-cert"] => dev_cert::dev_cert(),
        ["release"] => release::release(false),
        ["release", "--allow-ad-hoc"] => release::release(true),
        ["ci-keychain"] => ci_keychain::ci_keychain(false),
        ["ci-keychain", "--skip-if-unset"] => ci_keychain::ci_keychain(true),
        ["upload-release", tag] => upload_release::upload_release(tag),
        [] | ["help" | "--help" | "-h"] => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => Err(util::Error(format!(
            "unknown arguments {args:?}\n\n{USAGE}"
        ))),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
