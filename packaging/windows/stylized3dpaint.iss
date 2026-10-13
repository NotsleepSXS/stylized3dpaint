; Inno Setup script: Stylized3DPaint installer for Windows (x64).
; Built by CI:  iscc /DAppVersion=0.1.0 packaging\windows\stylized3dpaint.iss
#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#define AppName "Stylized3DPaint"
#define AppExe "stylized3dpaint.exe"
#define Root "..\.."

[Setup]
AppId={{6F2C2C8E-6D63-4B8A-9C5F-3D7A2E1B5A10}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=NotsleepSXS
AppPublisherURL=https://github.com/NotsleepSXS/stylized3dpaint
AppSupportURL=https://github.com/NotsleepSXS/stylized3dpaint/issues
AppUpdatesURL=https://github.com/NotsleepSXS/stylized3dpaint/releases
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
LicenseFile={#Root}\LICENSE-MIT
OutputDir={#Root}\dist
OutputBaseFilename=Stylized3DPaint-{#AppVersion}-windows-x64-setup
SetupIconFile={#Root}\assets\icon\stylized3dpaint.ico
UninstallDisplayIcon={app}\{#AppExe}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequiredOverridesAllowed=dialog
ChangesAssociations=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "assoc"; Description: "Open .s3dp projects with {#AppName}"; GroupDescription: "File types:"

[Files]
Source: "{#Root}\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\assets\hdri\LICENSE.md"; DestDir: "{app}"; DestName: "HDRI-LICENSE.md"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{group}\{cm:UninstallProgram,{#AppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
Root: HKA; Subkey: "Software\Classes\.s3dp"; ValueType: string; ValueName: ""; ValueData: "Stylized3DPaint.Project"; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Stylized3DPaint.Project"; ValueType: string; ValueName: ""; ValueData: "Stylized3DPaint project"; Flags: uninsdeletekey; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Stylized3DPaint.Project\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe},0"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\Stylized3DPaint.Project\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: assoc

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
