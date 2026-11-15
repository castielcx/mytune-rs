; Builds MyTune-rs-Setup-<version>.exe. Run installer\build.ps1 rather than compiling this directly.
#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#define AppExe "MyTune.exe"

[Setup]
; Never change AppId: it is how a newer setup recognises and upgrades an older install.
; (Different from the C# MyTune's id: this setup removes a C# install itself, see PrepareToInstall.)
AppId={{D81C9CBF-B63F-4355-B97C-5163CD45B323}
AppName=MyTune
AppVersion={#AppVersion}
AppVerName=MyTune {#AppVersion}
AppPublisher=MyTune
DefaultDirName={autopf}\MyTune rs
UsePreviousAppDir=yes
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName=MyTune
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

[InstallDelete]
; Setups up to 0.1.0 called the program mytune-ui.exe and its shortcuts "MyTune rs".
Type: files; Name: "{app}\mytune-ui.exe"
Type: files; Name: "{autoprograms}\MyTune rs.lnk"
Type: files; Name: "{autodesktop}\MyTune rs.lnk"

[Icons]
Name: "{autoprograms}\MyTune"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\MyTune"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "Launch MyTune"; Flags: nowait postinstall skipifsilent

[Code]
procedure StopApp;
var
  code: Integer;
begin
  // The app can sit in the tray: close it before files are replaced or removed. Applied tweaks stay in place.
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM {#AppExe}', '', SW_HIDE, ewWaitUntilTerminated, code);
  // The program's name in setups up to 0.1.0.
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM mytune-ui.exe', '', SW_HIDE, ewWaitUntilTerminated, code);
  Sleep(800);
end;

const
  UninstallKeys = 'SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\';
  // AppId of the C# MyTune (installer\MyTune.iss in the C# project). It never changes.
  OldCsId = '{B7E3C1A4-5D2F-4E8A-9C61-3F0A7D5E2B19}_is1';
  OwnId = '{D81C9CBF-B63F-4355-B97C-5163CD45B323}_is1';

var
  OldCsVersion, OldCsUninstaller, OldRsVersion: String;
  HadAutostart: Boolean;

// Version and uninstaller of an Inno Setup install, '' when it is not on this PC.
procedure FindInstall(id: String; var version, uninstaller: String);
begin
  version := '';
  uninstaller := '';
  // HKLM is the 64-bit view in this setup; HKLM32 covers a setup that ran in 32-bit mode.
  if not RegQueryStringValue(HKLM, UninstallKeys + id, 'UninstallString', uninstaller) then
    RegQueryStringValue(HKLM32, UninstallKeys + id, 'UninstallString', uninstaller);
  uninstaller := RemoveQuotes(uninstaller);
  // A registry entry whose uninstaller is gone is a leftover, not an install.
  if not FileExists(uninstaller) then
  begin
    uninstaller := '';
    Exit;
  end;
  if not RegQueryStringValue(HKLM, UninstallKeys + id, 'DisplayVersion', version) then
    RegQueryStringValue(HKLM32, UninstallKeys + id, 'DisplayVersion', version);
  if version = '' then version := '(unknown version)';
end;

function InitializeSetup: Boolean;
var
  unused: String;
begin
  FindInstall(OldCsId, OldCsVersion, OldCsUninstaller);
  FindInstall(OwnId, OldRsVersion, unused);
  Result := True;
end;

// Shown only when an older MyTune is on this PC. Silent installs skip the page and replace it all the same.
procedure InitializeWizard;
var
  found: String;
begin
  found := '';
  if OldCsVersion <> '' then found := 'MyTune C# ' + OldCsVersion;
  if OldRsVersion <> '' then
  begin
    if found <> '' then found := found + ' and ';
    found := found + 'MyTune rs ' + OldRsVersion;
  end;
  if found = '' then Exit;
  CreateOutputMsgPage(wpLicense,
    'Old ' + found + ' in system',
    'Setup overrides it and keeps your tweaks and settings.',
    'Setup removes the old version and installs MyTune {#AppVersion} in its place.' + #13#10#13#10 +
    'Tweaks you turned on stay applied to Windows. Your settings and the backups that undo the tweaks ' +
    'are kept, and the new version uses them.' + #13#10#13#10 +
    'Click Next to continue, or Cancel to leave this PC as it is.');
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  code, i: Integer;
begin
  Result := '';
  StopApp;
  // An older MyTune rs has the same AppId: its files are replaced in place, nothing to remove.
  if OldCsUninstaller = '' then Exit;
  // The C# uninstaller deletes the "Start with Windows" task: remember it, it is made again for the new exe.
  HadAutostart := Exec(ExpandConstant('{sys}\schtasks.exe'), '/Query /TN MyTune', '', SW_HIDE, ewWaitUntilTerminated, code) and (code = 0);
  // Run silently, its question "also delete saved settings?" answers No: state.json and the backups stay.
  // It closes a running MyTune.exe itself.
  if not Exec(OldCsUninstaller, '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART', '', SW_HIDE, ewWaitUntilTerminated, code) or (code <> 0) then
  begin
    Result := 'The old MyTune could not be removed (code ' + IntToStr(code) + '). ' +
      'Remove it in Windows Settings > Apps, then run this setup again. Your tweaks and settings are untouched.';
    Exit;
  end;
  // The uninstaller finishes in a copy of itself: wait until its file is gone.
  for i := 1 to 30 do
    if FileExists(OldCsUninstaller) then Sleep(500);
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  code: Integer;
begin
  if (CurStep = ssPostInstall) and HadAutostart then
    Exec(ExpandConstant('{app}\{#AppExe}'), '--enable-autostart', '', SW_HIDE, ewWaitUntilTerminated, code);
end;

function InitializeUninstall: Boolean;
begin
  StopApp;
  Result := True;
end;
// Settings and backups live in %ProgramData%\MyTune, shared with the C# MyTune, so uninstalling never deletes them.
