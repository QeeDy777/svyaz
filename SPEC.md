# Связь 0.4

Windows 10/11 x64 application for automatic selection of Flowseal zapret strategies,
using Discord API, Gateway Hello (opcode 10), and voice WebSocket Hello (opcode 8)
as the success criteria. The user explicitly accepted a responding voice server,
without testing audio. A TCP connection, DNS answer, HTTP error, or UDP send is
never treated as voice success. No Discord credentials are requested or read.

The existing conversation authorizes implementation, updates, and autostart.
Autostart means the current administrator's interactive Windows logon, with a
15-second delay, not a service before login. The UI remains in the tray.

## Components and order

1. Core: strict BAT-to-argument parsing, safe release extraction, protocol checks,
   selection and update transaction. No dependency on UI or Windows.
2. Windows: owned winws process, kill-on-close job, protected app data, Task Scheduler.
3. WPF/XAML Fluent UI: bindings, presentation state, connection orchestration,
   cancellation, results and settings. WinForms NotifyIcon handles only the tray.
4. Build: self-contained Windows executable and ZIP, assertions on macOS/Windows.

## Scope and constraints

- One managed winws process at a time. Existing external winws blocks selection;
  never kill other applications or delete their services.
- Parse supported general*.bat commands; do not execute downloaded BAT/PowerShell.
- Update complete official GitHub release ZIP, require GitHub asset SHA-256, bound
  downloads/extraction, reject traversal, symlinks and incompatible packages.
- Keep active and previous packages. Promote only after successful checks; on
  failure restore the prior active process. Preserve *-user.txt lists.
- Last successful strategy is tried first. Require two consecutive full passes.
- Discover the most recent voice endpoint in Discord/Canary/PTB text logs. Read
  bounded log tails, extract only discord.media host/port, never persist raw logs.
  If none exists, clearly request a voice connection attempt in Discord; do not
  silently fall back to API-only success. Optional explicit endpoint is available.
- Voice Hello only proves the voice control server responds, not UDP/audio or the
  availability of all Discord voice regions. Log format may change.
- If no voice address is known, automatically select a temporary strategy by
  API/Gateway reachability and wait up to a minute for a Discord voice-join attempt.
  This temporary state is not reported as verified voice connectivity.
- Auto-update at launch and every six hours; network errors keep cached package.
- Autostart enabled on first successful setup, visible switch, disable removes only
  our task. Stage EXE and five WPF native DLLs under Program Files/Versions, verify
  hashes, then switch the task. Keep data at its original protected location.
- Disable native library self-extraction to a per-user cache; the selected SDK
  supplies a statically linked single-file host. Retry startup up to five times.
- No system DNS/hosts/firewall edits, no antivirus changes, no account access.

## Commands and structure

`dotnet run --project tests/DiscordRoute.Checks`

`dotnet publish src/DiscordRoute.App -c Release -r win-x64 --self-contained true -o dist/win-x64`

`src/DiscordRoute.Core`: platform-independent logic; `src/DiscordRoute.App`: Windows
UI and process integration; `tests/DiscordRoute.Checks`: dependency-free assertions.
Use standard .NET APIs, PascalCase types, async with CancellationToken, explicit
errors at trust boundaries. Example: `if (!result.Success) return null;`.

## Verification

Assert malformed strategies, endpoints, Hello payloads, ZIP traversal/digest failures,
selection order, cancellation/cleanup and update rollback. Parse all strategies in
an actual upstream release. Compile and publish Windows x64. Test protocol I/O
against a local WebSocket server. Windows GUI, WinDivert and scheduled-task runtime
must be tested on a Windows machine; document the gap if none is available.

Always preserve previous working state and distinguish unknown from success.
Never read Discord tokens, execute arbitrary scripts, alter other project files,
publish externally without user authorization, or call unverified network access a completed voice connection.

## 0.2 migration acceptance

- Preserve settings, cached packages, voice discovery, updater and tray behavior.
- Use system light/dark/high-contrast Fluent resources, vector UI and three tabs.
- Keep failed, pending and successful checks distinct from process-running state.
- Initialize only once even if WPF Loaded repeats during theme changes.
- Deliver an explicitly unsigned ZIP and a signing guide. Optional signing must
  verify trust, selected publisher and RFC3161 timestamp before archiving.
- No SmartScreen bypass, protection disabling, or promise of prompt-free launch.

## 0.2.1 design refinement

User requested a fixed vertical rectangle, minimalist layout, Russian name and
an EXE icon. Product name: Связь. Window: 440 × 680 DIP, no resize/maximize;
retain system DPI scaling and scroll content when needed. One primary button
connects, cancels or disconnects according to state. History starts collapsed.
Embed the same multi-size icon in EXE, window and tray. Keep legacy data paths,
mutex and scheduled-task identity so the rename does not reset existing settings.

## Game Filter

Match Flowseal service.bat 1.10.4: Disabled/All/Tcp/Udp, default port ranges
1024-65535, legacy GameFilter maps to TCP except UDP-only mode. Disabled protocols
retain upstream placeholder 12. Validate all custom ports/ranges before saving.
Keep disabled for existing installations. Save only while disconnected; new
settings apply to the next connection and survive logon/package updates.

When enabled, resolve main ipset-all references to a protected shared list fetched
from Flowseal's .service/ipset-service.txt, as used by upstream Update IPSet List.
Do not change exclusions, original package lists, unrelated services or processes.
Use fixed HTTPS origin, no redirects, bounded fetch, CIDR validation and atomic
replacement. Recheck after six hours on connection; preserve valid cache on errors.
No cache means explicit failure, never empty/any-address filtering. Discord checks
do not prove game connectivity; state that in the settings and documentation.
# Telegram / tg-ws-proxy (0.4.0)

Отдельная вкладка Telegram, независимая от Discord. Официальный Windows x64 EXE
Flowseal tg-ws-proxy загружается при первом запуске с проверкой SHA-256 GitHub.
Прокси слушает только 127.0.0.1; случайный secret сохраняется между запусками.
Порт, запуск/остановка, открытие tg://proxy, копирование ссылки, ручное обновление
и необязательный запуск вместе со «Связью». При общем выходе завершаются оба
управляемых процесса. Чужой экземпляр не завершается и не перенастраивается.

Компонент работает в portable-режиме и сохраняет собственный трей. Его отдельные
автозапуск и автообновление отключены; обновления выполняет «Связь», при включённом
общем автообновлении — не чаще 6 часов и только когда прокси остановлен. Неудачная
новая версия не вытесняет предыдущую до проверки локального порта. Отмена не
запускает откат. Отображается готовность локального прокси, а не успех Telegram.
Подключение прокси подтверждается пользователем в Telegram; аккаунт не читается.

Основание: Flowseal/tg-ws-proxy v1.11.4, commit
6690b788432fd0f59b0f38e228494bcd6bede1ca; docs/RU/README.windows.md,
utils/tray_common.py, utils/default_config.py, windows.py. Лицензия MIT сохраняется.
