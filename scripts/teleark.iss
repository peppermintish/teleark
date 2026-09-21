; Inno Setup Script for TeleArk Desktop
#define AppId "{9A67D26D-7281-4FE9-B942-D6D2A8719DF5}"
#define AppName "TeleArk"
#ifndef AppVersion
  #define AppVersion "0.4.9"
#endif
#ifndef AppArchitecture
  #define AppArchitecture "x64compatible"
#endif
#define AppPublisher "TeleArk Contributors"
#define AppURL "https://github.com/Kangarooss/teleark"
#define AppExeName "teleark.exe"
#ifndef SourceDir
  #define SourceDir "..\dist\teleark-windows-x86_64"
#endif
#ifndef OutputDir
  #define OutputDir "..\dist"
#endif
#ifndef OutputBaseFilename
  #define OutputBaseFilename "TeleArk-Setup"
#endif

[Setup]
AppId={{#AppId}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}
AppUpdatesURL={#AppURL}
DefaultDirName={localappdata}\Programs\{#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir={#OutputDir}
OutputBaseFilename={#OutputBaseFilename}
SetupIconFile=..\crates\teleark-gui\assets\icons\teleark.ico
UninstallDisplayIcon={app}\{#AppExeName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
MinVersion=10.0.17763
ArchitecturesAllowed={#AppArchitecture}

; In-place upgrade configuration
UsePreviousAppDir=yes
UsePreviousGroup=yes
UsePreviousTasks=yes
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\{#AppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\{#AppName}"; Filename: "{app}\{#AppExeName}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(AppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

[Code]
function TryParseVersion(VersionText: String; var Version: Int64): Boolean;
begin
  VersionText := Trim(VersionText);
  { Older TeleArk installers used a leading v in DisplayVersion. }
  if (Length(VersionText) > 0) and ((VersionText[1] = 'v') or (VersionText[1] = 'V')) then
    Delete(VersionText, 1, 1);
  Result := StrToVersion(VersionText, Version);
  if not Result then
    Result := StrToVersion(VersionText + '.0', Version);
end;

function CheckInstalledVersion(const RootKey: Integer; const InstallerVersion: Int64): Boolean;
var
  InstalledText, MessageText: String;
  InstalledVersion: Int64;
begin
  Result := True;
  if not RegQueryStringValue(RootKey,
    'Software\Microsoft\Windows\CurrentVersion\Uninstall\{#AppId}_is1',
    'DisplayVersion', InstalledText) then
    Exit;

  if not TryParseVersion(InstalledText, InstalledVersion) then begin
    MessageText := 'TeleArk cannot determine the installed version (' + InstalledText + '). ' +
      'Installation has been stopped before any files were changed.';
    Log(MessageText);
    SuppressibleMsgBox(MessageText, mbCriticalError, MB_OK, IDOK);
    Result := False;
    Exit;
  end;

  if ComparePackedVersion(InstallerVersion, InstalledVersion) < 0 then begin
    MessageText := 'A newer version of TeleArk (' + InstalledText + ') is already installed.' + #13#10 +
      'This installer is version {#AppVersion}. Downgrading is not permitted.' + #13#10 +
      'Installation has been stopped before any files were changed.';
    Log(MessageText);
    SuppressibleMsgBox(MessageText, mbCriticalError, MB_OK, IDOK);
    Result := False;
  end;
end;

function InitializeSetup(): Boolean;
var
  InstallerVersion: Int64;
begin
  if not TryParseVersion('{#AppVersion}', InstallerVersion) then begin
    Log('Invalid TeleArk installer version: {#AppVersion}');
    Result := False;
    Exit;
  end;
  Result := CheckInstalledVersion(HKCU, InstallerVersion);
  if Result then
    Result := CheckInstalledVersion(HKLM, InstallerVersion);
end;
