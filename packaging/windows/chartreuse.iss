; Chartreuse's Windows installer, compiled with Inno Setup 6 (ISCC.exe) by
; `cargo xtask release` on Windows, which defines on ISCC's command line:
;
;   AppVersion          the Cargo.toml version
;   BundleId            the release bundle id, which names the value Open at
;                       login adds under the Run key
;   SourceDir           the staged release: chartreuse.exe (signed), LICENSE,
;                       README.md
;   IconFile            the installer's icon (the release app icon)
;   OutputDir           where the installer is written (target\dist)
;   OutputBaseFilename  Chartreuse-<version>-windows-<arch>[-unsigned]-setup
;   Sign                set when signing; the xtask then also passes the
;                       `chartreuse` sign tool (/Schartreuse=...), which signs
;                       the installer and the uninstaller inside it
;
; The install is per user: no administrator rights, into
; %LOCALAPPDATA%\Programs\Chartreuse, with a Start menu shortcut and an entry
; in Settings > Apps for the uninstaller. Installing a newer version over an
; older one upgrades it in place (same AppId, same folder), closing the
; running Chartreuse first. Uninstalling also takes Chartreuse off the
; programs Windows starts at login.

#ifndef AppVersion
  #error Build this with `cargo xtask release`, which defines AppVersion and the rest
#endif

[Setup]
; The installer's identity, which upgrades and the uninstaller find the
; installed copy by. Never change it.
AppId={{331B5945-F37F-4708-91CD-E3CBB3D53535}
AppName=Chartreuse
AppVersion={#AppVersion}
AppPublisher=Stephen Jennings
AppPublisherURL=https://github.com/jennings/chartreuse
AppSupportURL=https://github.com/jennings/chartreuse/issues
AppUpdatesURL=https://github.com/jennings/chartreuse/releases
PrivilegesRequired=lowest
DefaultDirName={autopf}\Chartreuse
DisableProgramGroupPage=yes
; x64 builds; also installs on Windows on Arm, which runs them emulated.
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Windows 10 1703: Per-Monitor DPI Awareness v2 (chartreuse.exe.manifest).
MinVersion=10.0.15063
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\chartreuse.exe
UninstallDisplayName=Chartreuse
WizardStyle=modern
Compression=lzma2
SolidCompression=yes
; Chartreuse keeps running in the tray, so its executable is in use during an
; upgrade or uninstall: close it through the Restart Manager, forcibly if it
; does not exit when asked.
CloseApplications=force
RestartApplications=no
OutputDir={#OutputDir}
OutputBaseFilename={#OutputBaseFilename}
#ifdef Sign
SignTool=chartreuse
#endif

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
Source: "{#SourceDir}\chartreuse.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Registry]
; Open at login (in Chartreuse's settings) adds a value named for the bundle
; id under the Run key, and Task Manager's Startup apps may add one of the
; same name under StartupApproved\Run. Delete both on uninstall, or Windows
; keeps trying to start the deleted executable at every login. Installing
; writes nothing (ValueType none, dontcreatekey).
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "{#BundleId}"; Flags: dontcreatekey uninsdeletevalue
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"; ValueType: none; ValueName: "{#BundleId}"; Flags: dontcreatekey uninsdeletevalue

[Icons]
Name: "{autoprograms}\Chartreuse"; Filename: "{app}\chartreuse.exe"

[Run]
Filename: "{app}\chartreuse.exe"; Description: "{cm:LaunchProgram,Chartreuse}"; Flags: nowait postinstall skipifsilent
