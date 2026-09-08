# Конфигурация

## Источник истины

Runtime: `Program Files\TandemWorkbench\config.json`. Образец: [`../config.example.json`](../config.example.json). Формат JSON выбран, чтобы использовать уже существующий `serde_json`, а не добавлять ещё один парсер.

`initialize` создаёт конфигурацию один раз. Неизвестные поля запрещены; отсутствующие поля получают безопасные defaults. Повреждённый JSON и неподдерживаемая версия схемы не подменяются фиктивным успехом.

```json
{
  "schema_version": 1,
  "game_filter": "disabled",
  "ipset_filter": "loaded",
  "check_updates_on_start": false,
  "request_timeout_secs": 8,
  "targets": [
    "https://www.youtube.com/",
    "https://discord.com/",
    "https://web.telegram.org/"
  ],
  "hosts_allowed_suffixes": [
    "discord.com",
    "discord.gg",
    "discord.media",
    "discordapp.net",
    "web.telegram.org"
  ]
}
```

| Поле | Default | Допустимые значения / семантика |
|---|---|---|
| `schema_version` | `1` | Только `1` |
| `game_filter` | `disabled` | `disabled`, `tcp`, `udp`, `all`; расширяет соответствующие порты до `1024-65535` |
| `ipset_filter` | `loaded` | `loaded` — сохранённый source; `none` — пустой active; `any` — IPv4/IPv6 `/0` |
| `check_updates_on_start` | `false` | Явная opt-in проверка GitHub-релиза при запуске GUI; не скачивает и не устанавливает пакет |
| `request_timeout_secs` | `8` | Целое `1..30` для одной HTTPS-проверки |
| `targets` | Три HTTPS-сервиса | `1..12` уникальных URL; HTTPS, DNS hostname, порт 443; без credentials, query и fragment |
| `hosts_allowed_suffixes` | Пять suffixes выше | `1..32` корректных DNS suffixes; точный hostname или поддомен по границе точки |

Не меняйте suffixes на публичные зоны вроде `com`: это ослабляет смысл allowlist. Добавление домена означает ваше явное разрешение на изменение его локального адреса, а не доверие любому полученному IP. Ввод IP должен пройти public-address policy.

## Применение

Проверка локального файла работает без Windows:

```sh
cargo run --locked -p tandem-core --bin tandem -- config-check config.example.json
```

На Windows, elevated PowerShell; сначала остановите службу:

```powershell
.\target\release\tandem.exe service stop
.\target\release\tandem.exe config-apply "$PWD\config.example.json"
.\target\release\tandem.exe service install "general.bat"
```

`config-apply` сохраняет конфигурацию и согласует IPSet под journal. Изменение `game_filter` требует переустановки службы: существующая командная строка SCM сама собой не меняется. GUI сообщает об этом явно. Простой `service start` запускает ранее установленную командную строку.

## IPSet без потери исходных данных

`lists/ipset-source.txt` хранит импортированный источник; `lists/ipset-all.txt` — активное представление. Последовательность `none → any → loaded` возвращает исходный список. Новый source import не меняет выбранный mode: в `none` active остаётся пустым, в `any` остаётся `/0`.

Разрешены IP и CIDR, пустые строки и `#`-комментарии. Дубли удаляются; ошибочные адреса и prefix lengths отклоняются до записи. Лимит — 8 MiB и 200 000 уникальных записей. Проверка CIDR здесь синтаксическая, не проверка принадлежности диапазона бизнес-сервису.

```powershell
.\target\release\tandem.exe service stop
.\target\release\tandem.exe ipset import "C:\Approved\ipset.txt"
.\target\release\tandem.exe service start
```

## Hosts

Hosts никогда не меняется в initialize/install. Для `hosts apply` нужен явный путь к проверенному локальному тексту. Адреса должны быть публичными, имена — в allowlist. Ввод с чужими management markers, конфликтующими hostname mappings, private/loopback/mapped IPv6 отклоняется.

Сохраняются комментарии и unmanaged content, включая CRLF. UTF-8 обязателен; старую ANSI/UTF-16 кодировку нужно предварительно исследовать и осознанно конвертировать, а не молча переписывать. Лимит — 2 MiB и 10 000 mappings. Повторное применение той же записи идемпотентно. Перед другой ревизией восстановите существующий recovery point.

## Переменные окружения

[`../.env.example`](../.env.example) — документация, **не автоматически загружаемый dotenv-файл**. Никакие секреты, токены или API keys не нужны.

| Переменная | Значение | Где используется |
|---|---|---|
| `TANDEM_QUIET` | `1` отключает вывод audit events в stderr | CLI/ядро; обязательный on-disk audit остаётся |
| `TANDEM_PREVIEW_PORT` | По умолчанию `5173`, диапазон `1..65535` | Только Node preview server |

```powershell
$env:TANDEM_QUIET = "1"
.\target\release\tandem.exe status
```

Для `cargo tauri dev` оставьте порт 5173: `tauri.conf.json` ожидает именно его. Ни environment variables, ни IPC не могут назначить произвольный каталог службы или executable.

## Политики, не отключаемые настройками

Обязательные SHA-256 и TLS verification; ограничение GitHub download origins; контроль прав root; единственная управляемая служба; отсутствие shell execution; ZIP path/size limits; запрет overwrite working engine при установленной службе; отдельный restore hosts. Эти свойства не спрятаны за `unsafe=true`.
