; Inno Setup script for the Windows installer.
;
; Built by .github/workflows/windows.yml:
;   iscc /DAppVersion=0.1.74 /DSourceExe=...\cosmic-mail.exe
;        /DOutputDir=...\dist packaging\windows\cosmic-mail.iss
;
; A per-user install (no administrator prompt): the app keeps its mail,
; settings and credentials per user anyway (%APPDATA%, Credential Manager).

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\..\target\release\cosmic-mail.exe"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

[Setup]
; Never change AppId: it is how upgrades find the existing install.
AppId={{6E0C1A55-2F3B-4D5B-9C8E-7A4C2B9F1D30}
AppName=Cosmic Mail
AppVersion={#AppVersion}
AppVerName=Cosmic Mail {#AppVersion}
; Windows file versions have four parts: MAJOR.MINOR.PATCH.0.
VersionInfoVersion={#AppVersion}.0
AppPublisher=PlusCosmic
AppPublisherURL=https://github.com/PlusCosmic/cosmic-mail
AppSupportURL=https://github.com/PlusCosmic/cosmic-mail/issues
DefaultDirName={autopf}\Cosmic Mail
DefaultGroupName=Cosmic Mail
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=cosmic-mail-{#AppVersion}-windows-x64-setup
SetupIconFile=..\..\assets\icon.ico
UninstallDisplayIcon={app}\cosmic-mail.exe
LicenseFile=..\..\LICENSE
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Close a running Cosmic Mail (it lives in the tray) before replacing it.
CloseApplications=force
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "startup"; Description: "Start Cosmic Mail in the background when I sign in (sync and notifications)"; GroupDescription: "Background:"

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "cosmic-mail.exe"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
; The toast icon: IconUri of the app's AppUserModelID (src/notifications.rs).
Source: "..\..\assets\icon-128.png"; DestDir: "{app}"; DestName: "icon.png"; Flags: ignoreversion

[Icons]
; AppUserModelID: notifications are posted under this id (src/notifications.rs), so
; Windows names them Cosmic Mail, not PowerShell. Never change it: Windows keeps the
; user's notification settings per id.
Name: "{autoprograms}\Cosmic Mail"; Filename: "{app}\cosmic-mail.exe"; AppUserModelID: "dev.pluscosmic.mail"
Name: "{autodesktop}\Cosmic Mail"; Filename: "{app}\cosmic-mail.exe"; Tasks: desktopicon

[Registry]
; Background start at sign-in: the Windows counterpart of the Linux
; cosmic-mail.service (--background: tray + sync, no window).
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Cosmic Mail"; ValueData: """{app}\cosmic-mail.exe"" --background"; Flags: uninsdeletevalue; Tasks: startup

; The notification identity. The app writes the same key itself on its first
; notification (so the portable zip gets it too); this entry is here so the
; uninstaller removes it.
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\dev.pluscosmic.mail"; ValueType: string; ValueName: "DisplayName"; ValueData: "Cosmic Mail"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\dev.pluscosmic.mail"; ValueType: string; ValueName: "IconUri"; ValueData: "{app}\icon.png"; Flags: uninsdeletekey

[Run]
Filename: "{app}\cosmic-mail.exe"; Description: "{cm:LaunchProgram,Cosmic Mail}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; The app keeps running in the tray after its window closes.
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM cosmic-mail.exe"; Flags: runhidden; RunOnceId: "StopCosmicMail"
