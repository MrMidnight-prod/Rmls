[Setup]
AppName=rmcl
AppVersion=0.1.0
DefaultDirName={autopf}\rmcl
DefaultGroupName=rmcl
OutputDir=output
OutputBaseFilename=rmcl-setup
Compression=lzma2
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=lowest

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"

[Files]
Source: "..\target\release\rmcl.exe"; DestDir: "{app}"
Source: "..\target\release\rmcl-gui.exe"; DestDir: "{app}"

[Icons]
Name: "{group}\rmcl"; Filename: "{app}\rmcl-gui.exe"
Name: "{autodesktop}\rmcl"; Filename: "{app}\rmcl-gui.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\rmcl-gui.exe"; Description: "Start rmcl"; Flags: nowait postinstall skipifsilent
