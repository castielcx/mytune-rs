; Builds MyTune-rs-Setup-<version>.exe. Run installer\build.ps1 rather than compiling this directly.
#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#define AppExe "mytune-ui.exe"

[Setup]
; Never change AppId: it is how a newer setup recognises and upgrades an older install.
; (Different from the C# MyTune's id on purpose, so both can be installed side by side.)
AppId={{D81C9CBF-B63F-4355-B97C-5163CD45B323}
AppName=MyTune rs
AppVersion={#AppVersion}
AppVerName=MyTune rs {#AppVersion}
AppPublisher=MyTune
DefaultDirName={autopf}\MyTune rs
UsePreviousAppDir=yes
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName=MyTune rs
SetupIconFile=..\crates\mytune-ui\assets\MyTune.ico
OutputDir=output
OutputBaseFilename=MyTune-rs-Setup-{#AppVersion}
VersionInfoVersion={#AppVersion}
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile=..\LICENSE

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\crates\mytune-ui\assets\twemoji\LICENSE-GRAPHICS.txt"; DestDir: "{app}"; DestName: "TWEMOJI-LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\MyTune rs"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\MyTune rs"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "Launch MyTune rs"; Flags: nowait postinstall skipifsilent

[Code]
procedure StopApp;
var
  code: Integer;
begin
  // The app can sit in the tray: close it before files are replaced or removed. Applied tweaks stay in place.
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM {#AppExe}', '', SW_HIDE, ewWaitUntilTerminated, code);
  Sleep(800);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopApp;
  Result := '';
end;

function InitializeUninstall: Boolean;
begin
  StopApp;
  Result := True;
end;
// Settings and backups live in %ProgramData%\MyTune, shared with the C# MyTune, so uninstalling never deletes them.
