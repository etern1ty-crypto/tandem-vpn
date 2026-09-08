# API / CLI справочник

## CLI conventions

Бинарник: `tandem` / `tandem.exe`. Команды принимают точное число аргументов; неизвестные команды и лишние аргументы отклоняются. Не запускаются произвольные shell commands. Native operations доступны только на Windows x64.

- stdout: JSON; исключения — `--help` и `--version`.
- stderr: JSON с ошибкой и audit events.
- 0 — команда выполнена; 2 — неверный ввод/JSON; 3 — недостаточные права или нарушение security policy; 4 — operation busy; 5 — системная/сетевая ошибка либо неподдерживаемая платформа.
- Успешно созданный `doctor`/`test-targets` report может содержать диагностические проблемы. Exit 0 означает «проверка выполнена», не «все сети исправны»; автоматизация должна анализировать поля результата.
- Файловые импорты/`config-apply` требуют абсолютного локального пути. UNC, device paths и traversal не принимаются. Offline inspect/config-check допускают путь к файлу, который затем canonicalize-ится.

## Офлайн-команды

```sh
tandem --help
tandem --version
tandem config-check config.example.json
tandem inspect "./fixtures/general.bat"
tandem bundle-check ./approved-release.zip "$APPROVED_SHA256" "$APPROVED_TAG"
```

- `config-check FILE`: читает реальный файл, отклоняет неверный JSON, unknown fields, ошибочные targets.
- `inspect FILE`: разбирает аргументы стратегии; файловые references показываются без требования существования. Ничего не запускает и не меняет. Путь к engine в плане — parent файла, поэтому Windows deployment plan нужно получить ещё раз на Windows через `preview`.
- `bundle-check ZIP SHA256 TAG`: проверяет pin, размер и ZIP metadata/path policy. Не устанавливает bundle и не доказывает исполнимость/подпись PE. Полная распаковка/CRC/layout checks происходят при import.

## Runtime-команды

| Команда | Admin | Изменение / результат |
|---|---|---|
| `init` | Да | Создать root/config, не устанавливая сервис |
| `status` | Нет | Текущий config, manifest, собственная служба, admin/read-only, recovery flags |
| `doctor` | Нет | BFE, службы WinDivert, собственная служба и диагностические codes |
| `support` | Нет | Минимальный обезличенный отчёт; не отправляет его никуда |
| `preview STRATEGY` | Нет | Проверенный Windows service plan и references |
| `config-apply FILE` | Да | Сохранить config + согласовать IPSet, только при stopped/absent службе |
| `service install STRATEGY` | Да | Проверить план/BFE/ownership, установить auto-start собственную службу |
| `service start` | Да | Запустить ранее установленную командную строку |
| `service stop` | Да | Остановить собственную службу; отсутствие идемпотентно |
| `service remove` | Да | Остановить/удалить только tandem-zapret |
| `bundle import ZIP SHA256 TAG` | Да | Проверить/распаковать/stage/promote пакет, только при absent службе |
| `bundle download SHA256 TAG` | Да | Выбрать один ZIP asset для указанного tag и проверить pin |
| `bundle rollback` | Да | Вернуть предыдущий managed пакет, только при absent службе |
| `ipset import FILE` | Да | Валидировать source и сохранить mode, только stopped/absent |
| `hosts apply FILE` | Да | Валидация + managed block + recovery point |
| `hosts restore` | Да | Вернуть исходный hosts при отсутствии внешнего конфликта |
| `recover` | Да | Service/deployment/settings recovery; hosts обрабатывается отдельно |
| `check-updates` | Нет | Проверить latest stable GitHub metadata; ничего не устанавливает |
| `test-targets` | Нет | До 12 HTTPS-проверок, до трёх параллельных workers |

Для `service install` остановите existing Tandem-service прежде, чем менять её аргументы. Наличие legacy-службы `zapret` не даёт разрешения её удалить: используйте owning tool/Services после административной проверки.

## Примеры безопасной автоматизации

```powershell
# Явно elevated PowerShell; digest/tag получены из вашей approved release policy.
$Cli = ".\target\release\tandem.exe"
& $Cli init
& $Cli bundle import "C:\Approved\release.zip" $ApprovedSha256 $ApprovedTag
& $Cli preview "general.bat"
& $Cli service install "general.bat"
& $Cli doctor
& $Cli support > ".\support.json"
```

В unattended automation проверяйте `$LASTEXITCODE` после каждого вызова. Выше — последовательность операций, не транзакционный PowerShell deployment script. Для fleet deployment нужны отдельные approval/rollback policies.

## Desktop IPC

Единственный Tauri command — `request`. Аргумент `action` десериализуется в `tandem_core::Action` с `deny_unknown_fields`. Ни arbitrary command, ни имя другой службы, ни другой executable/installation root передать нельзя.

```js
const status = await window.__TAURI__.core.invoke("request", {
  action: { type: "get_dashboard" }
});
const plan = await window.__TAURI__.core.invoke("request", {
  action: { type: "preview_strategy", strategy: "general.bat" }
});
```

| `action.type` | Дополнительные поля |
|---|---|
| `get_dashboard`, `initialize`, `diagnostics`, `export_support`, `check_updates`, `test_targets` | Нет |
| `save_config` | `config`: объект Config |
| `preview_strategy`, `install_service` | `strategy`: точное имя из `strategies` |
| `start_service`, `stop_service`, `remove_service`, `rollback_bundle`, `recover`, `restore_hosts` | Нет |
| `import_bundle` | `path`, `sha256`, `tag`: строки |
| `download_bundle` | `sha256`, `tag`: строки |
| `import_ipset`, `apply_hosts` | `path`: строка |

IPC success — JSON-значение; failure — rejected promise с текстом ошибки. Схема не является удалённым HTTP API: приложение не слушает публичный control port.

## Ключевые результаты

### `get_dashboard`

`schema_version`, `app_version`, `platform`, `root`, `initialized`, `administrator`, `config`, `bundle`, `strategies`, `service`, `service_owned`, `legacy_service_present`, `driver_present`, `pending_recovery`, `hosts_restore_available`, `bundle_rollback_available`.

Service snapshot содержит `state`, `binary_path`, `start_type`, `account`, `process_id`. Отсутствующая служба — `null`; access denied — ошибка, не `null`. Все состояния SCM представлены явно, включая pending/paused/unknown.

### `test_targets`

Каждая запись: `url`, `reachable`, `http_ok`, `status`, `elapsed_ms`, `error`.

- HTTPS response 403/404: `reachable=true`, `http_ok=false`, настоящий HTTP status сохранён.
- TLS/DNS/transport failure: `reachable=false`, `status=null`, `error` заполнено.
- Redirects не следуются; 3xx — ответ источника, а не доказательство успешной конечной загрузки.
- Это не speed test, не проверка видео/голоса и не доказательство DPI bypass.

### `check_updates`

`local_tag` берётся из manifest; `remote_tag` — stable release metadata. `different_release` означает различающийся tag, **не гарантированно более новую semver-версию**. `automatic_install=false`. `assets` содержит metadata; пользователь всё равно должен утверждать внешний digest.

### `export_support`

Включает версии, OS/arch, bundle tag/hash, game/IPSet modes, число targets и диагностические codes. Исключает конфигурационные URLs, local paths, IP, hosts, environment и raw logs. Самостоятельно никуда не загружается.
