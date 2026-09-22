; EXE setup wizard restored from the 2026-09-21 Inno installer.
; Windows Installer remains the sole owner of installed files and registration.
; All defines are supplied by build-windows-exe.ps1; no stale version fallback.
[Setup]
AppId=TeleArkSetupWizard
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=TeleArk Contributors
AppPublisherURL=https://github.com/Kangarooss/teleark
DefaultDirName={code:GetInstallDirectory}
PrivilegesRequired=lowest
OutputDir={#OutputDir}
OutputBaseFilename={#OutputBaseFilename}
SetupIconFile=..\crates\teleark-gui\assets\icons\teleark.ico
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
MinVersion=10.0.17763
ArchitecturesAllowed=x64compatible
DisableWelcomePage=no
DisableDirPage=no
DisableProgramGroupPage=yes
DisableReadyPage=no
DisableFinishedPage=no
UsePreviousAppDir=no
Uninstallable=no
CreateUninstallRegKey=no
CloseApplications=no
RestartApplications=no
SetupLogging=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

#include MessageFile

[Files]
Source: "{#MsiPath}"; DestName: "TeleArk.msi"; Flags: dontcopy

[Run]
Filename: "{app}\teleark.exe"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent; Check: CanLaunch

[Code]
var
  Installed, Installing, RestartRequired: Boolean;
  PreviousDirectory: String;

function GetInstallDirectory(Param: String): String;
begin
  if not RegQueryStringValue(HKCU64, '{#InstallerRegistryKey}', 'InstallLocation', Result) then
    if not RegQueryStringValue(HKCU64,
      'Software\Microsoft\Windows\CurrentVersion\Uninstall\{#LegacyAppId}_is1',
      'InstallLocation', Result) then
      Result := ExpandConstant('{localappdata}\Programs\{#AppName}');
end;

procedure InitializeWizard();
begin
  { Upgrades cannot relocate the existing product. Show the actual destination. }
  PreviousDirectory := '';
  if not RegQueryStringValue(HKCU64, '{#InstallerRegistryKey}', 'InstallLocation', PreviousDirectory) then
    RegQueryStringValue(HKCU64,
      'Software\Microsoft\Windows\CurrentVersion\Uninstall\{#LegacyAppId}_is1',
      'InstallLocation', PreviousDirectory);
  if PreviousDirectory <> '' then begin
    WizardForm.DirEdit.Text := PreviousDirectory;
    WizardForm.DirEdit.Enabled := False;
    WizardForm.DirBrowseButton.Enabled := False;
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Arguments, LogPath: String;
  ExitCode: Integer;
  Installer: Variant;
begin
  Result := '';
  if Installed then Exit;
  WizardForm.PreparingLabel.Caption := CustomMessage('InstallerPreparing');
  ExtractTemporaryFile('TeleArk.msi');
  LogPath := ExpandConstant('{log}') + '.msi.log';
  Arguments := '/i "' + ExpandConstant('{tmp}\TeleArk.msi') + '" /norestart /L*v "' +
    LogPath + '" INSTALLFOLDER="' + RemoveBackslashUnlessRoot(WizardDirValue()) + '"';
  { Basic native UI reports real progress and permits transactional cancellation.
    Only an explicit unattended request suppresses the installation UI. }
  if WizardSilent() then Arguments := Arguments + ' /qn'
  else Arguments := Arguments + ' /qb';
  Installer := CreateOleObject('WindowsInstaller.Installer');
  if Installer.ProductState('{#ProductCode}') = 5 then
    Arguments := Arguments + ' REINSTALL=ALL REINSTALLMODE=amus';
  Log('Starting installation. Windows Installer log: ' + LogPath);
  Installing := True;
  WizardForm.CancelButton.Enabled := False;
  try
    if not Exec(ExpandConstant('{sys}\msiexec.exe'), Arguments, '', SW_SHOW,
      ewWaitUntilTerminated, ExitCode) then begin
      Result := FmtMessage(CustomMessage('InstallerFailed'), [IntToStr(ExitCode), LogPath]);
      Exit;
    end;
  finally
    Installing := False;
    WizardForm.CancelButton.Enabled := True;
  end;
  Log('Windows Installer result: ' + IntToStr(ExitCode));
  if (ExitCode = 0) or (ExitCode = 3010) then begin
    Installed := True;
    RestartRequired := ExitCode = 3010;
  end else if ExitCode = 1602 then
    Result := CustomMessage('InstallerCancelled')
  else
    Result := FmtMessage(CustomMessage('InstallerFailed'), [IntToStr(ExitCode), LogPath]);
end;

procedure CancelButtonClick(CurPageID: Integer; var Cancel, Confirm: Boolean);
begin
  { The progress dialog owns cancellation while its transaction is active.
    Closing this wizard must not abandon a still-running installation. }
  if Installing then Cancel := False;
end;

function NeedRestart(): Boolean;
begin
  Result := RestartRequired;
end;

function CanLaunch(): Boolean;
begin
  Result := Installed and not RestartRequired;
end;
