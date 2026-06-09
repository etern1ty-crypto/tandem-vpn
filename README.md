# tandem-vpn

«Ультимативный» локальный обходной тандем для Windows — **без собственного сервера**.
Объединяет три независимых движка под одним GUI (Tauri + Rust):

| Фаза | Движок | Что делает |
|------|--------|------------|
| **1 (эта)** | **Zapret (Flowseal)** | Обход DPI на уровне пакетов (WinDivert). Все функции `service.bat` в GUI. |
| 2 (план) | **Cloudflare WARP** (`usque`) | Локальный SOCKS/HTTP-прокси через edge-сеть Cloudflare. |
| 3 (план) | **Goida (AvenCores)** | Тянет публичные конфиги с GitHub, чистит мусор, спид-тестит, оставляет топ-N. |

> **Нет главного сервера.** Приложение ничего не хостит: zapret работает локально,
> WARP идёт в сеть Cloudflare, Goida — в публичные ноды, конфиги обновляются прямым
> запросом к GitHub. Никакого нашего бэкенда или телеметрии.

## Архитектура

```
core/                 — чистая, кроссплатформенная, юнит-тестируемая логика (Rust)
  src/sys.rs          — абстракция запуска команд (sc/net/netsh/reg) + мок для тестов
  src/zapret/mod.rs   — установка/удаление службы, статус, диагностика, настройки
  src/zapret/strategy.rs — разбор стратегий .bat → аргументы winws.exe
app/                  — десктоп-приложение
  src-tauri/          — Rust-команды Tauri (обёртки над core + HTTP через ureq)
  src/, index.html    — фронтенд (Vite, vanilla JS)
```

Вся Windows-специфика (`sc`, `net`, `WinDivert`, реестр) изолирована за абстракцией
`Sys`, поэтому планирование команд тестируется на любой ОС. Сеть (проверка
обновлений, загрузка списков, тесты доступности) вынесена в слой Tauri — `core`
остаётся offline-тестируемым.

## Фаза 1 — функции Zapret GUI

Воспроизведены все пункты меню `service.bat`:

- **Install Service** — установка выбранной стратегии в автозапуск (через `sc create`).
- **Remove Services** — удаление `zapret` + `WinDivert`/`WinDivert14`, убийство `winws.exe`.
- **Check Status** — состояние служб, наличие `.sys`, запущен ли `winws.exe`, текущая стратегия.
- **Game Filter** — тумблер (применяется при установке: подстановка `%GameFilter*%`).
- **IPSet Filter** — `none` / `loaded` / `any`.
- **Auto-Update Check** — тумблер (`utils/check_updates.enabled`).
- **Update IPSet List** — загрузка актуального `ipset-all.txt` из репозитория Flowseal.
- **Check for Updates** — сравнение версии с `version.txt` апстрима.
- **Run Diagnostics** — проверка BFE, `.sys`, `winws.exe`, конфликтов.
- **Run Tests** — проверка доступности целей (`utils/targets.txt` или встроенный список).

> **Update Hosts File** (фикс веб-Telegram / Discord voice) пока не реализован — добавим
> после проверки источника на Windows.

## Сборка

Требования: Rust (stable), Node 18+, и для Windows-сборки — WebView2.

```bash
# фронтенд
cd app && npm install && npm run build

# проверка/тесты ядра (кроссплатформенно)
cargo test -p tandem-core

# запуск десктоп-приложения (нужен установленный tauri-cli: `cargo install tauri-cli`)
cd app && cargo tauri dev
```

`core` собирается и тестируется где угодно. Полноценная работа обхода требует
**Windows 11 + права администратора** (WinDivert — драйвер ядра), плюс распакованные
ассеты Zapret (`bin/`, `lists/`, стратегии `*.bat`) в папке установки.

## Лицензия

GPL-3.0-or-later. Проект использует наработки сообщества
([Flowseal/zapret-discord-youtube](https://github.com/Flowseal/zapret-discord-youtube),
[AvenCores/goida-vpn-configs](https://github.com/AvenCores/goida-vpn-configs)).
