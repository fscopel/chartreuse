# Icons

## Design

The mark is a capture selection: four selection corners around a crosshair, drawn in
near-black (`#161616`) with round caps. The app icon puts it on a rounded square in the
build flavor's accent color (`#80ff00` for release, `#f0cc00` for development, from
`chartreuse_core::flavor`), with a soft top-to-bottom sheen and a drop shadow, sized to
the macOS icon grid (an 824-unit body with a 185-unit corner radius on a 1024-unit
canvas). The accent tells development and release builds apart at a glance, in the
Finder and the tray.

## Sources

- `app-icon.svg` — the app icon. Its `accent` class carries the release accent; the xtask
  overrides it for the development flavor.
- `status-item-template.svg` — the mark alone, black on transparent, on an 18-unit canvas
  for the macOS menu bar. Its 2-unit strokes are centered on whole units with square ends,
  so it stays crisp at 1x and 2x.

Everything is rendered by `xtask/src/icon.rs` with resvg.

## Built at bundle and release time

Not checked in; rendered from `app-icon.svg` in the flavor's accent:

- macOS: `Contents/Resources/AppIcon.icns`, from an `.iconset` of every size from 16 to
  1024 pixels (`cargo xtask bundle`, `cargo xtask release`).
- Windows: nothing; the executable carries the checked-in app icon (below).
- Linux: `share/icons/hicolor/<size>x<size>/apps/io.jennings.chartreuse.png` in the
  release archive, from 16 to 512 pixels.

Up to 48 pixels, the Windows and Linux icons are the app icon's body cropped edge to edge,
without the drop shadow, like the tray icons below: the Windows shell and Linux panels
expect small icons to fill their image, and the margin and shadow would blur the mark.

## Compiled into the app: `generated/`

Checked in, so the app can `include_bytes!` or embed them without rendering SVG at build
time.
After editing an SVG, run `cargo xtask icons` to regenerate the directory; the xtask
tests fail while it is stale.

| File | Use |
| --- | --- |
| `status-item-template.png`, `status-item-template@2x.png` | macOS status item: 18 points at 1x (18 px) and 2x (36 px), loaded as a template `NSImage` |
| `app-release.ico`, `app-development.ico` | Windows app icon, embedded as the executable's icon resource by `crates/chartreuse/build.rs` (Explorer, the taskbar, and shortcuts show it); `app-release.ico` is also the installer's icon: PNG entries at 16, 24, 32, 48, 64, and 256 px |
| `tray-release.ico`, `tray-development.ico` | Windows notification-area icon (`Shell_NotifyIcon`): PNG entries at 16, 20, 24, 32, 40, and 48 px, the small icon from 100% to 300% scale |
| `tray-release-<size>.png`, `tray-development-<size>.png` | Linux tray (StatusNotifierItem `IconPixmap`): 16, 22, 24, 32, 48, and 64 px |

The tray icons are the app icon's body cropped edge to edge, without the drop shadow, so
the mark stays legible at 16 pixels. They are full color on any panel background. Pick the
file for `chartreuse_core::flavor::Flavor::CURRENT`, so a development build shows the
development accent.

For Windows, load the `.ico` from memory by choosing the entry nearest the small icon
size for the monitor's DPI (`GetSystemMetricsForDpi(SM_CXSMICON, dpi)`) and passing it
to `CreateIconFromResourceEx`; each entry is a complete PNG. For Linux, decode the PNGs
and convert them to the ARGB32 (network byte order) pixmaps StatusNotifierItem expects.
