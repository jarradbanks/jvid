; Per-user setup, one stable AppId across versions. User data survives uninstall.
#ifndef AppVersion
  #error AppVersion required
#endif
#ifndef StageDir
  #error StageDir required
#endif
#ifndef OutputDir
  #error OutputDir required
#endif
#include "whisper-models.iss"
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
SetupIconFile=..\assets\jvid.ico
LicenseFile={#StageDir}\DISTRIBUTION-LICENSE.txt

[Tasks]
Name: desktopicon; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs; Excludes: "jvid.portable,jvid-data\*"
; Models are user data: keep them across upgrades and uninstall.
Source: "{tmp}\{#Whisper0File}"; DestDir: "{localappdata}\jvid\data\models"; Flags: external ignoreversion skipifsourcedoesntexist uninsneveruninstall; Check: ShouldInstallModel(0)
Source: "{tmp}\{#Whisper1File}"; DestDir: "{localappdata}\jvid\data\models"; Flags: external ignoreversion skipifsourcedoesntexist uninsneveruninstall; Check: ShouldInstallModel(1)
Source: "{tmp}\{#Whisper2File}"; DestDir: "{localappdata}\jvid\data\models"; Flags: external ignoreversion skipifsourcedoesntexist uninsneveruninstall; Check: ShouldInstallModel(2)

[Icons]
Name: "{userprograms}\jvid"; Filename: "{app}\jvid-app.exe"; WorkingDir: "{app}"
Name: "{userdesktop}\jvid"; Filename: "{app}\jvid-app.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\jvid-app.exe"; Description: "Launch jvid"; Flags: nowait postinstall skipifsilent

[Code]
var
  ModelsPage: TInputOptionWizardPage;
  ModelDownloads: TDownloadWizardPage;
  ModelFiles: array[0..2] of String;
  ModelHashes: array[0..2] of String;
  ModelSizes: array[0..2] of Int64;
  ModelReady: array[0..2] of Boolean;
  ModelStatus: TNewStaticText;
  DownloadStarted: Int64;
  DownloadingModel: Integer;
  ModelDownloadError: String;

function GetTickCount64(): Int64;
  external 'GetTickCount64@kernel32.dll stdcall';

function VerifiedModel(const Path: String; Index: Integer): Boolean;
begin
  Result := False;
  if FileExists(Path) then
    try
      Result := CompareText(GetSHA256OfFile(Path), ModelHashes[Index]) = 0;
    except
      Log('Could not verify model: ' + ModelFiles[Index]);
    end;
end;

function ModelCachePath(Index: Integer): String;
begin
  Result := ExpandConstant('{localappdata}\jvid\cache\model-downloads\') +
    ModelHashes[Index] + '-' + ModelFiles[Index];
end;

procedure CacheModel(Index: Integer);
var
  Cache, Partial: String;
begin
  Cache := ModelCachePath(Index);
  Partial := Cache + '.partial';
  if ForceDirectories(ExtractFileDir(Cache)) and
    FileCopy(ExpandConstant('{tmp}\') + ModelFiles[Index], Partial, False) then
  begin
    { Publish only a completed copy; an interrupted cache is never reused. }
    DeleteFile(Cache);
    if not RenameFile(Partial, Cache) then
      Log('Could not retain completed model download: ' + ModelFiles[Index]);
  end
  else
    Log('Could not cache completed model download: ' + ModelFiles[Index]);
  DeleteFile(Partial);
end;

function ModelDownloadProgress(const Url, FileName: String;
  const Progress, ProgressMax: Int64): Boolean;
var
  Elapsed, Rate, Total, Remaining: Int64;
begin
  { Bound disk use even if the server sends an unexpected or unbounded response. }
  if (Progress > ModelSizes[DownloadingModel]) or
    ((ProgressMax > 0) and (ProgressMax <> ModelSizes[DownloadingModel])) then
  begin
    ModelDownloadError := 'The speech model download has an unexpected size.';
    Result := False;
    Exit;
  end;
  Total := ProgressMax;
  if Total <= 0 then Total := ModelSizes[DownloadingModel];
  Elapsed := GetTickCount64() - DownloadStarted;
  if (Progress > 0) and (Elapsed > 0) then
  begin
    Rate := Progress * 1000 div Elapsed;
    if Rate > 0 then
    begin
      Remaining := 0;
      if Total > Progress then Remaining := (Total - Progress) div Rate;
      ModelStatus.Caption := Format('%d / %d MB  |  %d KB/s  |  about %d min %d sec remaining', [
        Progress div 1000000, Total div 1000000, Rate div 1000,
         Remaining div 60, Remaining mod 60]);
    end;
  end
  else
    ModelStatus.Caption := 'Waiting for the model download server...';
  Result := True;
end;

function ShouldInstallModel(Index: Integer): Boolean;
begin
  Result := (not WizardSilent) and ModelsPage.Values[Index] and ModelReady[Index] and
    FileExists(ExpandConstant('{tmp}\') + ModelFiles[Index]);
end;

procedure InitializeWizard;
begin
  ModelFiles[0] := '{#Whisper0File}';
  ModelFiles[1] := '{#Whisper1File}';
  ModelFiles[2] := '{#Whisper2File}';
  ModelHashes[0] := '{#Whisper0SHA256}';
  ModelHashes[1] := '{#Whisper1SHA256}';
  ModelHashes[2] := '{#Whisper2SHA256}';
  ModelSizes[0] := {#Whisper0Size};
  ModelSizes[1] := {#Whisper1Size};
  ModelSizes[2] := {#Whisper2Size};
  ModelsPage := CreateInputOptionPage(wpSelectTasks, 'Optional speech models',
    'Add local transcription and subtitles',
    'Only one model is needed. Choose any models to download; internet access is required. ' +
    'Leave all unchecked to install without downloads. More models can be added later from Help > Optional Components.',
    False, False);
  ModelsPage.Add('Whisper Base - {#Whisper0MB} MB');
  ModelsPage.Add('Whisper Small - {#Whisper1MB} MB');
  ModelsPage.Add('Whisper Large v3 Turbo (Q5) - {#Whisper2MB} MB');
  ModelsPage.Values[0] := False;
  ModelsPage.Values[1] := False;
  ModelsPage.Values[2] := False;
  ModelDownloads := CreateDownloadPage('Downloading speech models',
    'Completed downloads are kept if you cancel or retry.', @ModelDownloadProgress);
  ModelDownloads.ShowBaseNameInsteadOfUrl := True;
  ModelStatus := TNewStaticText.Create(WizardForm);
  ModelStatus.Parent := ModelDownloads.Surface;
  ModelStatus.Left := 0;
  ModelStatus.Top := ModelDownloads.AbortButton.Top + ModelDownloads.AbortButton.Height + ScaleY(12);
  ModelStatus.Width := ModelDownloads.SurfaceWidth;
  ModelStatus.Height := ScaleY(48);
  ModelStatus.AutoSize := False;
  ModelStatus.WordWrap := True;
end;

function ShouldSkipPage(PageID: Integer): Boolean;
begin
  { In-app upgrades and unattended installs must never add model downloads. }
  Result := WizardSilent and (PageID = ModelsPage.ID);
end;

function DownloadModels(): Boolean;
var
  Index, Other: Integer;
  Existing, Temporary, Error: String;
  Shown: Boolean;
begin
  Result := True;
  if WizardSilent then Exit;
  ModelDownloadError := '';
  Shown := False;
  for Index := 0 to 2 do ModelReady[Index] := False;
  try
    try
      for Index := 0 to 2 do
        if ModelsPage.Values[Index] then
        begin
          Existing := ExpandConstant('{localappdata}\jvid\data\models\') + ModelFiles[Index];
          if not VerifiedModel(Existing, Index) then
          begin
            Temporary := ExpandConstant('{tmp}\') + ModelFiles[Index];
            if not VerifiedModel(Temporary, Index) then
            begin
              if VerifiedModel(ModelCachePath(Index), Index) then
              begin
                if not FileCopy(ModelCachePath(Index), Temporary, False) then
                  RaiseException('Could not read the completed model download cache.');
              end
              else
              begin
                DownloadingModel := Index;
                ModelDownloadError := '';
                DownloadStarted := GetTickCount64();
                ModelStatus.Caption := 'Connecting to the model download server...';
                ModelDownloads.Clear;
                ModelDownloads.Add('https://huggingface.co/ggerganov/whisper.cpp/resolve/' +
                  '{#WhisperRevision}/' + ModelFiles[Index], ModelFiles[Index], ModelHashes[Index]);
                if not Shown then
                begin
                  ModelDownloads.Show;
                  Shown := True;
                end;
                ModelDownloads.Download;
                { A click on the final progress update must still cancel this attempt. }
                if ModelDownloads.AbortedByUser then
                  RaiseException('The speech model download was cancelled.');
                if ModelDownloadError <> '' then RaiseException(ModelDownloadError);
              end;
            end;
            { Also verify a copied cache before making it eligible for installation. }
            if not VerifiedModel(Temporary, Index) then
              RaiseException('The speech model checksum could not be verified.');
            ModelReady[Index] := True;
            { Keep each verified model before starting the next large download. }
            CacheModel(Index);
          end;
        end;
    except
      Result := False;
      for Other := 0 to 2 do
        if not ModelReady[Other] then
          DeleteFile(ExpandConstant('{tmp}\') + ModelFiles[Other]);
      if (not Shown) or (not ModelDownloads.AbortedByUser) then
      begin
        Error := GetExceptionMessage;
        if ModelDownloadError <> '' then Error := ModelDownloadError;
        Log('Optional model download failed: ' + Error);
        if SuppressibleMsgBox('A speech model could not be downloaded.' + #13#10 + Error + #13#10#13#10 +
          'Continue installing with the completed models? Missing models can be added later from Help > Optional Components.',
          mbConfirmation, MB_YESNO, IDNO) = IDYES then
        begin
          for Other := 0 to 2 do
            if not ModelReady[Other] then ModelsPage.Values[Other] := False;
          Result := True;
        end;
      end
      else
        Log('Optional model download cancelled.');
    end;
  finally
    if Shown then ModelDownloads.Hide;
  end;
end;

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
  if Result and (CurPageID = wpReady) then
    Result := DownloadModels();
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  { Also enforce this if the directory wizard page was skipped. }
  if FileExists(ExpandConstant('{app}\jvid.portable')) then
    Result := 'Cannot install over a portable copy. Choose a different folder.';
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Config: String;
begin
  if (CurStep = ssPostInstall) and (not WizardSilent) then
  begin
    { The installer already offered models; keep management available from Help. }
    Config := ExpandConstant('{userappdata}\jvid\config');
    if ForceDirectories(Config) then
      SaveStringToFile(Config + '\components-offered', '1' + #13#10, False);
  end;
end;
