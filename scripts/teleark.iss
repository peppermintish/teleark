; Inno Setup Script for TeleArk Desktop
#define AppId "{9A67D26D-7281-4FE9-B942-D6D2A8719DF5}"
#define AppName "TeleArk"
#ifndef AppVersion
  #define AppVersion "0.4.4"
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
AppId={#AppId}
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
function ParseVersionComponent(var V: String): Integer;
var
  P: Integer;
begin
  P := Pos('.', V);
  if P > 0 then begin
    Result := StrToIntDef(Copy(V, 1, P - 1), 0);
    V := Copy(V, P + 1, Length(V));
  end else begin
    Result := StrToIntDef(V, 0);
    V := '';
  end;
end;

function CompareVersions(V1, V2: String): Integer;
var
  Num1, Num2: Integer;
begin
  while (Length(V1) > 0) or (Length(V2) > 0) do begin
    Num1 := ParseVersionComponent(V1);
    Num2 := ParseVersionComponent(V2);
    if Num1 > Num2 then begin
      Result := 1;
      Exit;
    end;
    if Num1 < Num2 then begin
      Result := -1;
      Exit;
    end;
  end;
  Result := 0;
end;

function InitializeSetup(): Boolean;
var
  InstalledVer: String;
  InstallerVer: String;
begin
  Result := True;
  InstallerVer := '{#AppVersion}';

  if RegQueryStringValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{#AppId}_is1', 'DisplayVersion', InstalledVer) or
     RegQueryStringValue(HKLM, 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{#AppId}_is1', 'DisplayVersion', InstalledVer) then
  begin
    if CompareVersions(InstallerVer, InstalledVer) < 0 then begin
      MsgBox(
        'A newer version of TeleArk (v' + InstalledVer + ') is already installed on this system.' + #13#10#13#10 +
        'This installer is version v' + InstallerVer + '.' + #13#10#13#10 +
        'Downgrading is not permitted. Installation will now abort.',
        mbError, MB_OK);
      Result := False;
      Exit;
    end;
  end;
end;
