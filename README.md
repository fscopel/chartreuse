# Chartreuse

A cross-platform screenshot and annotation tool that lives in the menu bar or the
system tray. See [PLAN.md](PLAN.md) for what it does and how it is built, and
[TASKS.md](TASKS.md) for the build checklist.

## Command line

```sh
chartreuse                     # start Chartreuse in the menu bar or tray
chartreuse capture display     # capture the whole desktop
chartreuse capture window      # capture the window you click
chartreuse capture rectangle   # capture the rectangle you drag
chartreuse open <file>         # open an image file in an editor window
chartreuse --help              # or --version
```

One Chartreuse runs per user (and per flavor: a development build runs beside a
release). A command given while it runs is handed to it, and the command line exits:
with status 0 once Chartreuse has taken the command, or 1 with the reason (a capture
already in progress, a file that cannot be opened); a usage error exits with 2. If
Chartreuse is not running, the command starts it, and that process stays running as
Chartreuse. Commands travel over a per-user channel:
`~/Library/Caches/<bundle id>/ipc.sock` on macOS,
`$XDG_RUNTIME_DIR/<bundle id>/ipc.sock` on Linux, and the named pipe
`\\.\pipe\<bundle id>-<user>` on Windows.

The command line is the executable itself. On macOS that is the one inside the
bundle, run directly or through a symlink:

```sh
ln -s /Applications/Chartreuse.app/Contents/MacOS/chartreuse /usr/local/bin/chartreuse
"target/debug/Chartreuse Dev.app/Contents/MacOS/chartreuse" capture window  # dev build
```

Starting the app from Finder, the Dock, `open`, or a login item goes through
LaunchServices, which never starts a second copy and does not pass `open --args`
arguments to a running one; use the executable for commands. Keep Chartreuse running
(for example as a login item) before using them: a copy started from a terminal runs
as the terminal's child, and macOS asks for, and checks, the terminal's Screen
Recording permission instead of Chartreuse's.

### Binding a desktop shortcut

Chartreuse registers global hotkeys itself where the platform lets it. Where it
cannot, notably on Wayland compositors without the GlobalShortcuts portal (such as
Sway and other wlroots compositors), bind a desktop shortcut to the command line.
Use the full path to the executable if it is not on the shortcut daemon's `PATH`.

- **GNOME**: *Settings → Keyboard → View and Customize Shortcuts → Custom Shortcuts →
  Add Shortcut*, with the command `chartreuse capture rectangle`.
- **KDE Plasma**: *System Settings → Keyboard → Shortcuts → Add New → Command or
  Script…*, with the command `chartreuse capture rectangle`, then assign the key.
  (Plasma 5: *Custom Shortcuts → Edit → New → Global Shortcut → Command/URL*.)
- **Sway** (`~/.config/sway/config`):

  ```
  bindsym $mod+Shift+4 exec chartreuse capture rectangle
  ```

- **Hyprland** (`~/.config/hypr/hyprland.conf`):

  ```
  bind = SUPER SHIFT, 4, exec, chartreuse capture rectangle
  ```

- **Windows**: right-click the desktop, *New → Shortcut*, with the location
  `"C:\path\to\chartreuse.exe" capture rectangle`. In the shortcut's *Properties*,
  set a *Shortcut key* (Windows makes it Ctrl+Alt+*key*), and *Run: Minimized*, since
  the command line is a console program and would flash a console window. Windows
  honors shortcut keys only for shortcuts on the desktop or in the Start menu folder.
- **macOS**: Chartreuse's own hotkeys cover this. Launchers such as Shortcuts or
  Raycast can run the bundle's executable as above.

## Platform support

macOS is the primary platform. The Windows and Linux backends are implemented, except
for layer-shell overlays and `ext-image-copy-capture` window capture on Wayland (see
[Wayland](#wayland)), and CI builds and tests them, but they have not yet been tried
on real machines: the limitations below come from the APIs, protocols, and portals
they use. The [integration pass checklists](TASKS.md#4a--windows-m8) in TASKS.md list
what to try.

| | macOS | Windows | Linux, X11 | Linux, Wayland |
|---|---|---|---|---|
| Tray | Menu bar item | Notification-area icon | StatusNotifierItem | StatusNotifierItem |
| Hotkeys | Carbon hot keys | `RegisterHotKey` | `XGrabKey` | GlobalShortcuts portal |
| Display capture | ScreenCaptureKit | Windows.Graphics.Capture, else `BitBlt` | MIT-SHM | Screenshot portal |
| Rectangle selection | Overlay per display | Overlay per display | Overlay per display | Full-screen overlay; the portal's picker on several displays |
| Window selection | Overlay per display | Overlay per display | Overlay per display | The portal's picker |
| Window capture | ScreenCaptureKit, with shadow | Windows.Graphics.Capture, else `PrintWindow`; no shadow | XComposite | The portal's picker |
| Clipboard | `NSPasteboard` | PNG and `CF_DIBV5` | `CLIPBOARD` selection | Data-control protocols; XWayland on GNOME |
| File dialogs | `NSOpenPanel`, `NSSavePanel` | `IFileOpenDialog`, `IFileSaveDialog` | FileChooser portal | FileChooser portal |
| Settings file | `~/Library/Application Support/<bundle id>/settings.toml` | `%APPDATA%\<bundle id>\settings.toml` | `$XDG_CONFIG_HOME/<bundle id>/settings.toml` (`~/.config`) | same as X11 |

The bundle id is `io.jennings.chartreuse` for releases and `io.jennings.chartreuse.dev`
for development builds.

### Windows

- Windows 10 or later. Overlays keep out of captures only on Windows 10 2004 and
  later (`WDA_EXCLUDEFROMCAPTURE`); earlier, a capture started while one is on screen
  shows it. Other Chartreuse windows on screen (editors, settings) are captured.
- Window captures have no drop shadow (DWM draws it outside the window, where neither
  capture API reaches), and on Windows 11 their rounded corners are transparent.
- Monitors of mixed DPI: each monitor's logical size is its pixel size divided by its
  own scale, and neighbours are laid edge to edge, so in an L or a ring of mixed
  scales logical rectangles can overlap slightly. Overlays are placed in pixels, over
  their monitor.
- Chartreuse is a console program, so that the command line reports to the terminal
  and its exit status is waited for. Started from Explorer, a shortcut, or a login
  item, it closes the console window Windows gives it; the window may flash.
- The Per-Monitor DPI Awareness v2 manifest is embedded only by the MSVC toolchain;
  GNU builds fall back to winit's DPI awareness.

### Linux

Chartreuse picks the backend the way winit does: Wayland when `WAYLAND_DISPLAY` (or
`WAYLAND_SOCKET`) is set, else X11. Unsetting `WAYLAND_DISPLAY` runs it under
XWayland with the X11 backend, which sees only XWayland's windows and pixels.

- Tray: needs a StatusNotifierItem host. KDE Plasma, Xfce, Cinnamon, MATE, LXQt,
  Budgie, and Waybar have one; GNOME needs the AppIndicator extension. There is no
  XEmbed fallback: without a host Chartreuse says the tray icon is unavailable, and
  is reached through its hotkeys and the command line.
- File dialogs: need xdg-desktop-portal with a FileChooser backend (GNOME, KDE, or
  `xdg-desktop-portal-gtk`; xdg-desktop-portal-wlr has none, so install the GTK one
  beside it).
- Hotkeys that another program holds are reported (X11) or left to the desktop to
  resolve (Wayland).

#### X11

- One scale factor for the whole desktop: the `Xft.dpi` setting every major desktop
  makes (or `WINIT_X11_SCALE_FACTOR`). Without it winit gives each monitor a factor
  of its own from its physical size, and overlays on monitors whose factor differs
  from the primary's are misplaced.
- Window capture needs a compositing manager to capture a window whole. Without one
  a window is read from the screen, with whatever covers it.

#### Wayland

No Wayland client may read other clients' pixels, list their windows, or place its
own windows, so the desktop's portals do much of the work:

- Display capture takes the Screenshot portal's screenshot of the whole desktop,
  which the desktop may ask the user to allow the first time. A refusal is reported.
  The screenshot is assumed to show the desktop's logical layout at one scale, as
  GNOME's and grim's do.
- Rectangle selection: winit offers no layer-shell, so the overlay is a full-screen
  window on the output the compositor chooses. That covers a single-output desktop;
  with several outputs, rectangle captures go to the portal's own picker instead.
- Window selection is the portal's own picker.
- The portal saves each screenshot as a file (GNOME in the Pictures folder), which
  Chartreuse deletes once read.

| | GNOME | KDE Plasma | Sway, Hyprland, and others |
|---|---|---|---|
| Portal picker (window captures; rectangles on several outputs) | GNOME's screenshot UI: an area, a window, or a screen | The portal's dialog | xdg-desktop-portal-wlr has none and captures the whole desktop; other portals vary |
| Hotkeys | GlobalShortcuts portal, GNOME 48 and later | GlobalShortcuts portal | Hyprland's portal has it; on Sway and others [bind a shortcut](#binding-a-desktop-shortcut) |
| Clipboard | Through XWayland | `ext-data-control` / `wlr-data-control` | `ext-data-control` / `wlr-data-control` |
| Tray | AppIndicator extension | Built in | Panels with a tray (Waybar, …) |

The GlobalShortcuts portal may ask the user to confirm the hotkeys or pick others; its
failures are only logged.

## Development setup (macOS)

macOS ties the Screen Recording permission to an app's code signature, so
Chartreuse always runs as a signed `.app` bundle, launched through LaunchServices.
See [macOS code signing](PLAN.md#macos-code-signing) for the background.

### 1. Tools

- [rustup](https://rustup.rs). The toolchain version is pinned in
  `rust-toolchain.toml` and installed automatically.
- Xcode Command Line Tools (`xcode-select --install`) for `codesign` and `iconutil`
  (and, for releases, `lipo`, `notarytool`, and `stapler`).

### 2. A signing identity

macOS remembers the Screen Recording permission for a build's signing identity.
Builds signed ad-hoc (the fallback) have none: to macOS every rebuild is a new app, so
the permission never sticks, and the app does not ask for it (macOS would ask again on
every launch). Set up an identity once, either way below.

**`cargo xtask dev-cert` (recommended).** It creates a self-signed code-signing
certificate in a keychain of its own, `~/Library/Keychains/chartreuse-dev-signing.keychain-db`,
without any prompt and without touching your login keychain. `cargo xtask bundle` and
`cargo xtask run` sign with it whenever `CHARTREUSE_SIGN_IDENTITY` is unset, so the
builds of every checkout and jj workspace are one app to macOS. Once per machine:

```sh
cargo xtask dev-cert
# macOS keeps one Screen Recording record per bundle id, pinned to the build that
# first asked. If any earlier build asked (ad-hoc, or signed with another
# certificate), no build signed with this identity matches it, so macOS would ask
# on every launch. Drop it:
tccutil reset ScreenCapture io.jennings.chartreuse.dev
# macOS asks once: allow Chartreuse Dev in System Settings, then relaunch.
cargo xtask run
```

The keychain's password is fixed and not a secret, so any program running as you can
sign code with this identity and so inherit Chartreuse Dev's Screen Recording
permission (as with a login-keychain identity that `codesign` may *Always Allow*).
Remove it with `security delete-keychain
~/Library/Keychains/chartreuse-dev-signing.keychain-db`. A recreated identity is a new
certificate, so the old record matches no new build: run `cargo xtask dev-cert`, then
`tccutil reset ScreenCapture io.jennings.chartreuse.dev`, then allow the permission again.

**Your own certificate**, in your login keychain:

- **Apple Development** (free with an Apple ID): Xcode → Settings → Accounts → add
  your Apple ID → Manage Certificates → **+** → Apple Development.
- **Self-signed**: Keychain Access → Certificate Assistant → Create a Certificate…,
  with Identity Type *Self-Signed Root* and Certificate Type *Code Signing*.

List the identities `codesign` can use, and point the build at yours:

```sh
security find-identity -v -p codesigning
export CHARTREUSE_SIGN_IDENTITY="Apple Development: Your Name (TEAMID1234)"
```

The value is the quoted name (or the 40-character hash) from `security
find-identity`. Put it in your shell profile. It takes precedence over the
`dev-cert` identity. Without either, builds are signed ad-hoc and a loud warning
says so.

### 3. Build and run

```sh
cargo xtask run
```

This builds `target/debug/Chartreuse Dev.app`, signs it, and launches it with `open`,
with the app's logs on your terminal. The app opens no windows at startup: it lives
in the menu bar, and the status item's Quit quits it. Set `RUST_LOG` (for example
`RUST_LOG=debug`) to change the log level.

`cargo xtask run --fake` (or `CHARTREUSE_BACKEND=fake cargo xtask run`) swaps in the
synthetic platform backend (fake displays, windows, and captures) for UI work without
real capture. It calls no macOS privacy API, so it never makes macOS prompt. The fake
status item is not a real menu bar icon, so quit that build with
`pkill -f 'Chartreuse Dev.app/Contents/MacOS'`.

### Automated work (AI agents, scripts)

Unattended work must not put a Screen Recording prompt in front of the person at
the Mac:

- `cargo xtask check`, `cargo test`, and `cargo xtask bundle` never prompt. Tests only
  read the permission status, and macOS attributes them to the terminal.
- Launch the app with `cargo xtask run --fake`.
- Plain `cargo xtask run` (the real backend, for example to try hotkeys or the status
  item) does not prompt from an ad-hoc build. A `dev-cert` build asks macOS once
  per identity: leave that first launch, and its answer, to the person (step 2).
- Examples that capture (`cargo run -p chartreuse-platform --example
  capture_displays`) and `screencapture` run as the terminal app, so macOS asks on
  the terminal's behalf if it lacks Screen Recording.
- Never run `tccutil reset`.

### Troubleshooting

- `codesign -d -r- "target/debug/Chartreuse Dev.app"` prints the designated
  requirement. It should name the bundle identifier and your certificate; a `cdhash`
  means the build was signed ad-hoc.
- A signed build that asks for Screen Recording on every launch: macOS's record for
  `io.jennings.chartreuse.dev` was made by another identity (usually an ad-hoc
  build). `tccutil reset ScreenCapture io.jennings.chartreuse.dev` clears it.
- The same command resets the grant to test the first-run flow. That flow needs a
  signed build, since ad-hoc builds never ask.
- With a self-signed certificate from Keychain Access, the keychain may ask for
  access to the private key on every build; choose *Always Allow* for `codesign`.
  (`dev-cert` sets up its key so that it never asks.)

## Commands

Everything beyond `cargo build` is a `cargo xtask` command, and CI runs nothing else.

| Command | Result |
|---|---|
| `cargo xtask check` | `cargo fmt --check`, `cargo clippy` with warnings denied, `cargo test` |
| `cargo xtask bundle` | Signed `target/debug/Chartreuse Dev.app` (macOS) |
| `cargo xtask run` | `bundle`, then launch it through LaunchServices. `--fake` uses the synthetic platform backend |
| `cargo xtask dev-cert` | Once per machine (macOS): create the self-signed development signing identity that `bundle` uses when `CHARTREUSE_SIGN_IDENTITY` is unset |
| `cargo xtask release` | Release build for the host platform, archived into `target/dist/` (emptied first) as `Chartreuse-<version>-<os>-<arch>`. macOS: a disk image (`.dmg`) of a universal (Apple silicon and Intel) `Chartreuse.app`, signed with `CHARTREUSE_RELEASE_SIGN_IDENTITY` (a Developer ID Application identity), notarized, stapled, and verified; see [From a developer machine](#from-a-developer-machine-macos). Windows (`.zip`): the executable, with the app icon embedded, signed with `signtool` and verified; see [Windows signing](#windows-signing). Linux (`.tar.gz`): the executable with the app icon. Both with `LICENSE` and `README.md`. `--allow-ad-hoc`: when no signing identity (macOS) or certificate (Windows) is set, sign the app ad-hoc (macOS) and skip signing the disk image and notarization, or sign nothing (Windows); the names end in `-unsigned`. |
| `cargo xtask ci-keychain` | CI (macOS): import the Developer ID identity from `CHARTREUSE_SIGN_P12_BASE64` (a base64-encoded `.p12`) and `CHARTREUSE_SIGN_P12_PASSWORD` into a temporary keychain that `codesign` uses without prompting, and print it. `--skip-if-unset` succeeds without doing anything when `CHARTREUSE_SIGN_P12_BASE64` is unset. Remove the keychain afterwards with the `security delete-keychain` command it prints (GitHub-hosted runners are discarded anyway). |
| `cargo xtask upload-release <tag>` | Attach every file in `target/dist/` to the GitHub release `<tag>` with the [GitHub CLI](https://cli.github.com) (`gh`), replacing assets of the same name. The tag must be `v<version>` for the `Cargo.toml` version, and every file must be named for that version. Needs `GH_TOKEN` (or `gh auth login`), and `GH_REPO=owner/repo` outside a git checkout. |

The build flavor (development or release: bundle identifier, name, accent color) is
chosen by the `release-flavor` cargo feature, which only `cargo xtask release`
enables; it is independent of the optimization profile.

## Cutting a release

GitHub Actions builds every platform and attaches the archives to the release
([`.github/workflows/release.yml`](.github/workflows/release.yml)):

1. Set `version` under `[workspace.package]` in `Cargo.toml` to the new version,
   and push that commit.
2. Create the release with a tag named `v<version>` (e.g. `v0.2.0`) on that commit,
   and publish it straight away: on GitHub, *Releases → Draft a new release*, then
   *Publish release* (tick *Set as a pre-release* for a trial run), or
   `gh release create v0.2.0 --target main --generate-notes` (`--prerelease` for a
   trial). Do not *Save draft* first: GitHub runs no workflows for drafts, and the
   workflow runs when a release is created, not when a draft is published.
3. The *Release* workflow builds on macOS, Windows, and Linux, then attaches:
   - `Chartreuse-<version>-macos-universal.dmg`: Apple silicon and Intel, signed with
     Developer ID and notarized. Until the [signing secrets](#release-signing-secrets)
     exist it is `Chartreuse-<version>-macos-universal-unsigned.dmg`, **ad-hoc
     signed** and not notarized: Gatekeeper blocks it on first launch; allow it under
     *System Settings → Privacy & Security → Open Anyway*.
   - `Chartreuse-<version>-windows-x86_64.zip`, signed; until the
     [signing secrets](#release-signing-secrets) exist,
     `Chartreuse-<version>-windows-x86_64-unsigned.zip`, which SmartScreen warns about
     on first run
   - `Chartreuse-<version>-linux-x86_64.tar.gz`

A tag that does not match the `Cargo.toml` version fails the upload, naming both.
Re-running a failed job replaces that platform's assets. Running the workflow by hand
(*Actions → Release → Run workflow*) is a dry run: it builds a branch on every
platform and keeps the archives as workflow artifacts, attaching nothing. Without
GitHub Actions, run `cargo xtask release` and then `cargo xtask upload-release
v<version>` on each platform (on macOS, set up signing first:
[From a developer machine](#from-a-developer-machine-macos)).

### Release signing secrets

The macOS job signs and notarizes once these repository secrets exist (*Settings →
Secrets and variables → Actions*). Without `CHARTREUSE_RELEASE_SIGN_IDENTITY` it
builds the `-unsigned` disk image; with it, a missing notarization secret fails the
job rather than publishing an app Gatekeeper rejects. The Windows job signs once
`CHARTREUSE_WINDOWS_SIGN_PFX_BASE64` exists, and builds `-unsigned` files until then.

| Secret | Value |
|---|---|
| `CHARTREUSE_SIGN_P12_BASE64` | The Developer ID Application certificate and its private key, exported from Keychain Access (*My Certificates*, select both, *Export 2 items…*) as a `.p12` with a password, then `base64 -i identity.p12` |
| `CHARTREUSE_SIGN_P12_PASSWORD` | That `.p12`'s password |
| `CHARTREUSE_RELEASE_SIGN_IDENTITY` | The identity's name (`Developer ID Application: Your Name (TEAMID)`) or SHA-1 hash, as `security find-identity -v -p codesigning` lists it |
| `CHARTREUSE_NOTARY_KEY` | The contents of an App Store Connect API key file (`AuthKey_<key ID>.p8`: App Store Connect → *Users and Access → Integrations → App Store Connect API*, role *Developer*) |
| `CHARTREUSE_NOTARY_KEY_ID` | That key's ID |
| `CHARTREUSE_NOTARY_ISSUER` | The issuer ID shown above the list of keys |
| `CHARTREUSE_WINDOWS_SIGN_PFX_BASE64` | A code signing certificate and its private key as a `.pfx`, base64-encoded (`base64 -w0 certificate.pfx`; PowerShell: `[Convert]::ToBase64String([IO.File]::ReadAllBytes("certificate.pfx"))`). See [Windows signing](#windows-signing) |
| `CHARTREUSE_WINDOWS_SIGN_PFX_PASSWORD` | That `.pfx`'s password |
| `CHARTREUSE_WINDOWS_SIGN_TIMESTAMP_URL` | Optional: an RFC 3161 timestamp server, if not DigiCert's `http://timestamp.digicert.com` |

### From a developer machine (macOS)

`cargo xtask release` runs the whole macOS release locally, exactly as CI does.
Once per machine:

1. Get a **Developer ID Application** certificate (a paid Apple Developer Program
   membership): Xcode → *Settings → Accounts → Manage Certificates → + → Developer ID
   Application*. `security find-identity -v -p codesigning` then lists it.
2. Store notarization credentials in a keychain profile, from an App Store Connect API
   key (see [the secrets](#release-signing-secrets)) or an Apple ID with an
   app-specific password:

   ```sh
   xcrun notarytool store-credentials chartreuse \
     --key AuthKey_ABC123.p8 --key-id ABC123 --issuer 69a6de7e-...
   # or: xcrun notarytool store-credentials chartreuse --apple-id you@example.com --team-id TEAMID
   ```

Then, for each release:

```sh
export CHARTREUSE_RELEASE_SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export CHARTREUSE_NOTARY_PROFILE=chartreuse
cargo xtask release
cargo xtask upload-release v0.2.0   # attach target/dist/* to the GitHub release
```

Instead of a profile, `CHARTREUSE_NOTARY_KEY_ID`, `CHARTREUSE_NOTARY_ISSUER`, and
either `CHARTREUSE_NOTARY_KEY_PATH` (the `.p8` file) or `CHARTREUSE_NOTARY_KEY` (its
contents) select an API key directly. A missing or conflicting variable fails the
release, before anything is built, with a message naming it.

`release` builds `aarch64-apple-darwin` and `x86_64-apple-darwin` (adding the Intel
target with `rustup target add` the first time) and merges them with `lipo`; signs
`Chartreuse.app` with the hardened runtime, a secure timestamp, and
`assets/macos/Chartreuse.entitlements`; notarizes and staples the app, so the copy
users drag out of the disk image carries its own ticket; builds the disk image with
`hdiutil`, signs, notarizes, and staples it; and finally verifies both with
`codesign --verify --strict`, `spctl`, and `stapler validate`. The two notarization
round trips usually take a few minutes each. The result is
`target/dist/Chartreuse-<version>-macos-universal.dmg`.

Without a certificate, `cargo xtask release --allow-ad-hoc` builds the same disk image
from an ad-hoc signed app, unsigned and not notarized, as
`Chartreuse-<version>-macos-universal-unsigned.dmg`: useful for testing the packaging,
not for distribution.

### Windows signing

`cargo xtask release` on Windows signs the executable with `signtool` from the newest
installed Windows SDK (or the one on `PATH`), using a SHA-256 digest and an RFC 3161
timestamp, then checks it with `signtool verify /pa` (a trusted chain, so a
self-signed test certificate fails the release). It takes the certificate from one of:

- `CHARTREUSE_WINDOWS_SIGN_CERT_SHA1`: the thumbprint of a code signing certificate
  in your personal certificate store (*certmgr.msc → Personal → Certificates →
  Details → Thumbprint*), for a hardware token or a signing service whose key
  storage provider puts the certificate there.
- `CHARTREUSE_WINDOWS_SIGN_PFX_BASE64` and `CHARTREUSE_WINDOWS_SIGN_PFX_PASSWORD`: a
  base64-encoded `.pfx` and its password, as in CI.

Setting both, or half of the second, fails the release before anything is built. With
neither, `--allow-ad-hoc` builds unsigned `-unsigned` files; without it the release
fails, naming the variables.

## Repository layout

| Path | Contents |
|---|---|
| `crates/chartreuse` | The app: `iced::daemon`, one module per feature |
| `crates/chartreuse-core` | Shared types and pure logic (geometry, images, hotkeys, errors, build flavor) |
| `crates/chartreuse-platform` | Platform traits, one backend directory per OS with one file per trait, and a `fake` backend |
| `crates/chartreuse-imaging` | Pixel operations, encoding, and decoding |
| `crates/chartreuse-config` | Settings schema and persistence |
| `crates/chartreuse-overlay` | Selection overlay canvas programs |
| `crates/chartreuse-editor` | Editor document model, tools, and canvas |
| `assets` | Icon sources and generated icons; `assets/macos/Chartreuse.entitlements`, the entitlements every macOS build is signed with (none, deliberately) |
| `xtask` | Build automation (`cargo xtask`) |
