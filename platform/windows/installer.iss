; Compile with Inno Setup 6 after staging target/package/windows.
#ifndef AppVersion
  #error AppVersion must be supplied
#endif
#ifndef NativeVersion
  #error NativeVersion must be supplied
#endif
#ifndef Arch
  #error Arch must be supplied: x64 or arm64
#endif
; x64compatible also admits Arm64 Windows 11, which runs x64 apps under
; emulation; the arm64 installer is native there.
#if Arch == "x64"
  #define Architectures "x64compatible"
#elif Arch == "arm64"
  #define Architectures "arm64"
#else
  #error Arch must be x64 or arm64
#endif

[Setup]
AppId={{B3F4C53F-77C7-4639-9938-C01A21F4A0B9}
AppName=Yours
AppVersion={#AppVersion}
VersionInfoVersion={#NativeVersion}
AppPublisher=Flectar
AppPublisherURL=https://flectar.com
DefaultDirName={localappdata}\Programs\Yours
PrivilegesRequired=lowest
ArchitecturesAllowed={#Architectures}
ArchitecturesInstallIn64BitMode={#Architectures}
MinVersion=10.0
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\yours.exe
SetupIconFile=..\..\resources\app-icon\yours.ico
OutputDir=..\..\target
OutputBaseFilename=flectar-mail-windows-{#Arch}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
; Tell Windows about the mail link handler below on install and uninstall.
ChangesAssociations=yes

[Files]
Source: "..\..\target\package\windows\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

; Offered as the default email app in Settings > Apps > Default apps. Only
; the user can make it the default, so the shared mailto key is left alone.
; The link is the whole command line after the option, as
; src/mailto_links.rs requires. src/default_email_app.rs writes the same
; layout under other names for the portable build.
[Registry]
Root: HKA; Subkey: "Software\Classes\FlectarMail.Url.mailto"; ValueType: string; ValueName: ""; ValueData: "Yours email link"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\FlectarMail.Url.mailto"; ValueType: string; ValueName: "FriendlyTypeName"; ValueData: "Yours email link"
Root: HKA; Subkey: "Software\Classes\FlectarMail.Url.mailto\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\yours.exe"",0"
Root: HKA; Subkey: "Software\Classes\FlectarMail.Url.mailto\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\yours.exe"" --mailto ""%1"""
Root: HKA; Subkey: "Software\Clients\Mail\Yours"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Clients\Mail\Yours\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "Yours"
Root: HKA; Subkey: "Software\Clients\Mail\Yours\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "Native, offline-first email client"
Root: HKA; Subkey: "Software\Clients\Mail\Yours\Capabilities"; ValueType: string; ValueName: "ApplicationIcon"; ValueData: """{app}\yours.exe"",0"
Root: HKA; Subkey: "Software\Clients\Mail\Yours\Capabilities\URLAssociations"; ValueType: string; ValueName: "mailto"; ValueData: "FlectarMail.Url.mailto"
Root: HKA; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "Yours"; ValueData: "Software\Clients\Mail\Yours\Capabilities"; Flags: uninsdeletevalue
; Windows that open links belong to the app's taskbar button.
Root: HKA; Subkey: "Software\Classes\FlectarMail.Url.mailto"; ValueType: string; ValueName: "AppUserModelID"; ValueData: "Flectar.FlectarMail"

; New-mail notifications: the class Windows starts the app through when a
; notification or one of its buttons is clicked, also after the app has quit.
; The Start menu shortcut below names it with the app id the toasts use; the
; app checks this command to tell the installed copy from a portable one.
; Both are in src/notifications/windows.rs.
Root: HKA; Subkey: "Software\Classes\CLSID\{{ED445996-CA23-4190-9817-4AABA2693036}"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\CLSID\{{ED445996-CA23-4190-9817-4AABA2693036}\LocalServer32"; ValueType: string; ValueName: ""; ValueData: """{app}\yours.exe"" --toast-activated"

[Icons]
Name: "{userprograms}\Yours"; Filename: "{app}\yours.exe"; AppUserModelID: "Flectar.FlectarMail"; AppUserModelToastActivatorCLSID: "ED445996-CA23-4190-9817-4AABA2693036"

[Run]
Filename: "{app}\yours.exe"; Description: "Open Yours"; Flags: nowait postinstall skipifsilent

[Code]
// The app registers its own per-user sign-in entry; remove it with the app so
// Windows does not keep a startup item that points at a deleted executable.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
  begin
    RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Yours');
    RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run', 'Yours');
  end;
end;
