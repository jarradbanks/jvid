; Per-user setup, one stable AppId across versions. Data is never an installer-owned file.
#ifndef AppVersion
  #error AppVersion required
#endif
#ifndef StageDir
  #error StageDir required
#endif
#ifndef OutputDir
  #error OutputDir required
#endif
[Setup]
AppId={{E8D1AB61-17F0-4C1B-8822-82A410FE435A}
AppName=jvid
AppVersion={#AppVersion}
AppPublisher=jarradbanks
AppPublisherURL=https://github.com/jarradbanks/jvid
DefaultDirName={localappdata}\Programs\jvid
DefaultGroupName=jvid
DisableDirPage=no
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=jvid-{#AppVersion}-windows-x86_64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\jvid-app.exe
CloseApplications=yes
RestartApplications=no
SetupMutex=jvid-setup
LicenseFile={#StageDir}\DISTRIBUTION-LICENSE.txt

[Tasks]
Name: desktopicon; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs; Excludes: "jvid.portable,jvid-data\*"

[Icons]
Name: "{userprograms}\jvid"; Filename: "{app}\jvid-app.exe"; WorkingDir: "{app}"
Name: "{userdesktop}\jvid"; Filename: "{app}\jvid-app.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\jvid-app.exe"; Description: "Launch jvid (optional speech models can be added on first run)"; Flags: nowait postinstall skipifsilent

[Code]
function InitializeSetup(): Boolean;
begin
  Result := True;
  { A portable tree has its own data contract. Do not turn it into an installed copy. }
end;

function NextButtonClick(CurPageID: Integer): Boolean;
begin
  Result := True;
  if (CurPageID = wpSelectDir) and FileExists(ExpandConstant('{app}\jvid.portable')) then
  begin
    Log('Refusing to install over a portable copy.');
    if not WizardSilent then
      MsgBox('Choose a different folder. This folder contains a portable copy of jvid.', mbError, MB_OK);
    Result := False;
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  { Also enforce this if the directory wizard page was skipped. }
  if FileExists(ExpandConstant('{app}\jvid.portable')) then
    Result := 'Cannot install over a portable copy. Choose a different folder.';
end;
