#ifndef AppVersion
  #error AppVersion is required
#endif
#ifndef Variant
  #define Variant "unsigned"
#endif
[Setup]
AppId={{D00D54B4-A1F3-4BC6-9F68-4B42D64502A9}
AppName=Связь
AppVersion={#AppVersion}
AppPublisher=QeeDy777
AppPublisherURL=https://github.com/QeeDy777/svyaz
AppSupportURL=https://github.com/QeeDy777/svyaz/issues
DefaultDirName={autopf}\DiscordRoute
DisableDirPage=yes
UsePreviousAppDir=no
DefaultGroupName=Связь
DisableProgramGroupPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64os
ArchitecturesInstallIn64BitMode=x64os
MinVersion=10.0.17763
OutputDir=..\dist
OutputBaseFilename=Svyaz-{#AppVersion}-win-x64-setup-{#Variant}
SetupIconFile=..\src\DiscordRoute.App\Assets\app.ico
UninstallDisplayIcon={app}\Application\Связь.exe
UninstallDisplayName=Связь
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
WizardResizable=no
WizardSizePercent=100
DisableWelcomePage=no
DisableReadyPage=yes
AppMutex=Global\DiscordRoute
CloseApplications=no
RestartApplications=no

[Languages]
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[Messages]
WelcomeLabel1=На связи.%nБез лишнего.
WelcomeLabel2=Установим «Связь» и добавим ярлык в меню «Пуск».%n%nDiscord, Telegram и обновления — в одном спокойном окне.%n%nВаши настройки сохранятся. Для компонентов подключения понадобится интернет при первом запуске.
FinishedLabel=«Связь» установлена. Можно подключаться.%n%nВ дальнейшем приложение будет обновляться автоматически, если обновления включены в настройках.
SetupAppRunningError=«Связь» ещё работает. Выберите «Выход (остановить всё)» в меню значка рядом с часами, затем продолжите установку.
UninstallAppRunningError=«Связь» ещё работает. Выберите «Выход (остановить всё)» в меню значка рядом с часами, затем продолжите удаление.

[Tasks]
Name: desktopicon; Description: "Ярлык на рабочем столе"; Flags: unchecked

[Files]
Source: "..\dist\win-x64\*"; DestDir: "{app}\Application"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Связь"; Filename: "{app}\Application\Связь.exe"; WorkingDir: "{app}\Application"
Name: "{autodesktop}\Связь"; Filename: "{app}\Application\Связь.exe"; WorkingDir: "{app}\Application"; Tasks: desktopicon

[Run]
Filename: "{app}\Application\Связь.exe"; Description: "Открыть «Связь»"; Flags: nowait postinstall skipifsilent runascurrentuser

[UninstallDelete]
Type: filesandordirs; Name: "{app}\Versions"
Type: files; Name: "{app}\Data\AppUpdate\current.json"
Type: files; Name: "{app}\Data\AppUpdate\pending.json"
Type: files; Name: "{app}\Data\AppUpdate\rejected.json"
Type: files; Name: "{app}\Data\AppUpdate\rollback.json"

[Code]
procedure InitializeWizard;
begin
  WizardForm.Color := $F0F6F5;
  WizardForm.MainPanel.Color := $F9FDFC;
  WizardForm.InnerPage.Color := $F9FDFC;
  WizardForm.WelcomePage.Color := $F9FDFC;
  WizardForm.FinishedPage.Color := $F9FDFC;
  WizardForm.WizardSmallBitmapImage.Visible := False;
  WizardForm.RunList.Left := ScaleX(32);
  WizardForm.WizardBitmapImage.Visible := False;
  WizardForm.WizardBitmapImage2.Visible := False;
  WizardForm.WelcomeLabel1.Left := ScaleX(32);
  WizardForm.WelcomeLabel1.Width := WizardForm.WelcomePage.Width - ScaleX(64);
  WizardForm.WelcomeLabel1.Height := ScaleY(104);
  WizardForm.WelcomeLabel2.Left := ScaleX(32);
  WizardForm.WelcomeLabel2.Top := WizardForm.WelcomeLabel1.Top + ScaleY(120);
  WizardForm.WelcomeLabel2.Width := WizardForm.WelcomeLabel1.Width;
  WizardForm.WelcomeLabel2.Height := WizardForm.WelcomePage.Height - WizardForm.WelcomeLabel2.Top - ScaleY(16);
  WizardForm.FinishedHeadingLabel.Left := ScaleX(32);
  WizardForm.FinishedLabel.Left := ScaleX(32);
  WizardForm.Font.Name := 'Segoe UI';
  WizardForm.Font.Color := $272B18;
  WizardForm.WelcomeLabel1.Font.Color := $516525;
  WizardForm.WelcomeLabel1.Font.Size := 28;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var Code: Integer;
begin
  if CurStep = ssPostInstall then
    if not Exec(ExpandConstant('{app}\Application\Связь.exe'), '--install', '', SW_HIDE, ewWaitUntilTerminated, Code) or (Code <> 0) then
      RaiseException('Не удалось завершить установку. Закройте «Связь» через значок в трее и повторите установку. Подробности: {app}\Data\startup-error.log');
end;

function InitializeUninstall: Boolean;
var Code: Integer;
begin
  Result := Exec(ExpandConstant('{app}\Application\Связь.exe'), '--uninstall', '', SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 0);
  if not Result then
    MsgBox('Не удалось отключить автозапуск. Закройте «Связь» через значок в трее и повторите удаление. Настройки и файлы сохранены.', mbError, MB_OK);
end;
