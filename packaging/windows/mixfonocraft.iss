; Installer Inno Setup di Mixfonocraft AI Studio.
;
; Compilazione (dalla radice del repository):
;   ISCC.exe /DAppVersion=0.1.0 /DSourceDir="C:\percorso\stage" packaging\windows\mixfonocraft.iss
;
; SourceDir deve contenere: mixfonocraft-studio.exe, mixfonocraft.exe, mfc-plugin-host.exe,
; README.md, examples\ (cartella). Output: dist\Mixfonocraft-<ver>-windows-x86_64-setup.exe

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\dist\stage-windows-x86_64"
#endif

#define AppName "Mixfonocraft AI Studio"
#define StudioExe "mixfonocraft-studio.exe"
#define CliExe "mixfonocraft.exe"
; Helper per i plugin VST3/CLAP in un processo separato: deve stare accanto all'app.
#define PluginHostExe "mfc-plugin-host.exe"

[Setup]
; AppId fisso: identifica l'applicazione per aggiornamenti e disinstallazione. Non cambiarlo.
AppId={{EAC91ABF-E6BE-47D7-8A11-1F3BFFB4A612}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Mixfonocraft
AppPublisherURL=https://github.com/basoxxx/Mixfonocraft-ai-studio
AppSupportURL=https://github.com/basoxxx/Mixfonocraft-ai-studio
AppUpdatesURL=https://github.com/basoxxx/Mixfonocraft-ai-studio/releases
AppComments=DAW in Rust con server MCP per agenti AI
VersionInfoVersion={#AppVersion}.0
VersionInfoProductName={#AppName}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
UninstallDisplayName={#AppName}
UninstallDisplayIcon={app}\{#StudioExe}
SetupIconFile=..\..\assets\icon\icon.ico
OutputDir=..\..\dist
OutputBaseFilename=Mixfonocraft-{#AppVersion}-windows-x86_64-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
; Installazione per utente o per tutto il computer: l'utente sceglie nella finestra iniziale.
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog
ChangesAssociations=yes
ChangesEnvironment=yes

[Languages]
Name: "italian"; MessagesFile: "compiler:Languages\Italian.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Crea un'icona sul desktop"; GroupDescription: "Icone aggiuntive:"; Flags: unchecked
Name: "addtopath"; Description: "Aggiungi la CLI 'mixfonocraft' al PATH (per il server MCP da terminale)"; GroupDescription: "Strumenti da riga di comando:"; Flags: unchecked

[Files]
Source: "{#SourceDir}\{#StudioExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\{#CliExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\{#PluginHostExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\examples\*"; DestDir: "{app}\examples"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#StudioExe}"; WorkingDir: "{app}"
Name: "{group}\Disinstalla {#AppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#StudioExe}"; WorkingDir: "{app}"; Tasks: desktopicon

[Registry]
; Associazione del formato .mfc (progetto nativo, JSON) all'app.
; HKA = HKLM con installazione per tutto il computer, HKCU con installazione per utente.
Root: HKA; Subkey: "Software\Classes\.mfc"; ValueType: string; ValueName: ""; ValueData: "MixfonocraftProject"; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\MixfonocraftProject"; ValueType: string; ValueName: ""; ValueData: "Progetto Mixfonocraft"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\MixfonocraftProject\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#StudioExe},0"
Root: HKA; Subkey: "Software\Classes\MixfonocraftProject\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#StudioExe}"" ""%1"""

[Run]
Filename: "{app}\{#StudioExe}"; Description: "Avvia {#AppName}"; WorkingDir: "{app}"; Flags: nowait postinstall skipifsilent

[Code]
const
  EnvSubkeyUser = 'Environment';
  EnvSubkeyMachine = 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';

procedure EnvRoot(var RootKey: Integer; var SubKey: string);
begin
  if IsAdminInstallMode then
  begin
    RootKey := HKEY_LOCAL_MACHINE;
    SubKey := EnvSubkeyMachine;
  end
  else
  begin
    RootKey := HKEY_CURRENT_USER;
    SubKey := EnvSubkeyUser;
  end;
end;

// True se la cartella non è già presente nel PATH dell'ambito di installazione.
function PathNeedsDir(Dir: string): Boolean;
var
  RootKey: Integer;
  SubKey, CurPath: string;
begin
  EnvRoot(RootKey, SubKey);
  if not RegQueryStringValue(RootKey, SubKey, 'Path', CurPath) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(CurPath) + ';') = 0;
end;

procedure AddDirToPath(Dir: string);
var
  RootKey: Integer;
  SubKey, CurPath, NewPath: string;
begin
  EnvRoot(RootKey, SubKey);
  if not RegQueryStringValue(RootKey, SubKey, 'Path', CurPath) then
    CurPath := '';
  if CurPath = '' then
    NewPath := Dir
  else
    NewPath := CurPath + ';' + Dir;
  RegWriteExpandStringValue(RootKey, SubKey, 'Path', NewPath);
end;

// Rimuove dal PATH tutte le voci uguali a Dir (confronto senza distinzione di maiuscole).
procedure RemoveDirFromPath(Dir: string);
var
  RootKey: Integer;
  SubKey, CurPath, Rest, Part, NewPath: string;
  P: Integer;
begin
  EnvRoot(RootKey, SubKey);
  if not RegQueryStringValue(RootKey, SubKey, 'Path', CurPath) then
    exit;
  NewPath := '';
  Rest := CurPath;
  while Rest <> '' do
  begin
    P := Pos(';', Rest);
    if P = 0 then
    begin
      Part := Rest;
      Rest := '';
    end
    else
    begin
      Part := Copy(Rest, 1, P - 1);
      Rest := Copy(Rest, P + 1, Length(Rest) - P);
    end;
    if (Part <> '') and (Uppercase(RemoveBackslash(Part)) <> Uppercase(RemoveBackslash(Dir))) then
    begin
      if NewPath <> '' then
        NewPath := NewPath + ';';
      NewPath := NewPath + Part;
    end;
  end;
  if NewPath <> CurPath then
    RegWriteExpandStringValue(RootKey, SubKey, 'Path', NewPath);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then
  begin
    if PathNeedsDir(ExpandConstant('{app}')) then
      AddDirToPath(ExpandConstant('{app}'));
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveDirFromPath(ExpandConstant('{app}'));
end;
