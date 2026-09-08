# Отчёт о проверках — 2026-09-08

## Краткий вердикт

**Frontend собран и проверен. Native Rust/Windows часть не подтверждена компилятором или реальной Windows-системой.** Поставка содержит реализованные исходники и regression tests, но не готовый подписанный EXE и не доказательство отсутствия всех дефектов.

Среда ревизии: Linux, `v24.14.1`, `Google Chrome for Testing 153.0.8010.12`, Python 3.13. `cargo` и `rustc` отсутствуют; DNS-доступ к static.rust-lang.org и registry.npmjs.org в sandbox не работал. Публичные web-страницы были доступны отдельным исследовательским инструментом, что не давало пакетному менеджеру сетевого доступа.

## Выполнено локально

| Проверка | Результат | Что именно доказано |
|---|---|---|
| `npm run check` | PASS | Синтаксис 7 JS modules проверен Node |
| `npm test` | PASS — 12/12 | Helpers, gate, validation, build и реальный localhost server |
| `npm run build` | PASS | Созданы реальные frontend files без внешних npm packages |
| Browser harness | PASS — 15/15 | Настоящий frontend в Chromium; IPC заменён явно помеченным test fixture |
| Visual QA | PASS | Просмотрены desktop, 390px, dark, error и confirmation states |
| Palette contrast | PASS — 20 pairs | Light/dark text tokens >=4.5:1; не полный WCAG audit |
| Public frontend resources | PASS | Нет CDN/runtime remote assets; preview не симулирует Windows backend |
| Documentation/source structure | Отдельный `local-checks.json` | Пути, версии, lock graph, DOM references; не компиляция |
| Original license preservation | PASS | LICENSE побайтово совпадает с исходным GPL-файлом |

Полные Node logs: [`../verification/frontend-checks.log`](../verification/frontend-checks.log).
Браузерные assertions: [`../verification/browser-results.json`](../verification/browser-results.json).
Повторяемый harness: [`../verification/browser.mjs`](../verification/browser.mjs).
Структурная проверка: [`../scripts/verify_repository.py`](../scripts/verify_repository.py).

### Что проверил браузер

Отключение native actions без Tauri; загрузку реальных модулей без JS exceptions; раскрытие advanced section; отсутствие horizontal overflow на 1100px и 390px; inert text rendering недоверенного имени стратегии; обязательное подтверждение и отсутствие IPC после отмены; refresh возвращённого состояния; ошибки backend и снятие busy; замену stale success JSON на актуальный error result; блокировку конкурирующих controls; реальное скачивание JSON; dialog на 390px в dark mode.

`TEST-FIXTURE` snapshots не доказывают, что Windows-служба действительно запустилась. Это проверка frontend boundary. В production-source нет browser mock fallback.

## Написано, но не выполнено

**44 Rust unit/regression tests** находятся непосредственно в production modules и CLI. `MockSys` применяется только для имитации системной границы. Они покрывают парсер, path policy, checksum/ZIP metadata, hosts merge/restore, IPSet mode round trip, config rollback и жизненный цикл собственной службы.

| Проверка | Статус / причина |
|---|---|
| `cargo fmt`, native Rust parser | NOT RUN — rustfmt отсутствует |
| `cargo metadata --locked` | NOT RUN — Cargo отсутствует |
| `cargo test --locked -p tandem-core --all-targets` | NOT RUN — Cargo отсутствует |
| Tauri/Windows compilation, Clippy | NOT RUN — нет Rust/Windows toolchain |
| Реальные SCM / ACL / rollback / reboot | NOT RUN — sandbox не Windows |
| Реальная загрузка/установка Flowseal/WinDivert | NOT RUN — нет нужной сети, Windows, независимо утверждённого package hash |
| Полный актуальный Cargo advisory scan | NOT RUN — нет Cargo/advisory network |
| Подпись установщика / проверка driver signatures | NOT RUN — signing identity не предоставлена; настоящего пакета не устанавливали |

Наличие `ci.yml`/`release.yml` означает подготовленную процедуру, **не прошедшую CI**. `Cargo.lock` проверен как статический граф сохранённых исходных registry packages; это не замена Cargo resolution.

## Обязательная Windows acceptance matrix

- [ ] Сборка core/CLI/desktop на чистой Windows 11 x64; canonical `cargo fmt` output сохранён.
- [ ] Все authored Rust tests, CLI smoke, Clippy correctness/suspicious и свежий cargo-audit — зелёные.
- [ ] Обычный токен читает status, но не меняет service/files; elevated token проходит допустимые операции.
- [ ] Default ACL, inherited ACL, user-owned/writable files, unknown ACEs, junctions, reparse points и не-ASCII paths проверены.
- [ ] Настоящий approved ZIP проходит import; bad hash/oversize/traversal/case-collision корректно отклоняются без изменения active engine.
- [ ] Service create/start/stop/remove, failed startup rollback, existing foreign service, service marked-for-deletion, paused/pending состояния.
- [ ] Reboot с auto-start и последующее безопасное удаление собственной службы.
- [ ] Kill процесса на каждой фазе promotion/settings/service update; повторный recover восстанавливает согласованность.
- [ ] Hosts CRLF/UTF-8, стороннее изменение, read-only/AV locks и невозможность потерять чужие mappings.
- [ ] Конкретные нужные сети/приложения, включая voice/QUIC/anti-cheat там, где это разрешено.
- [ ] Code signing, независимый review Win32 boundary, provenance сторонних binaries и правовой допуск.

До выполнения этих пунктов нельзя достоверно назвать пакет production-ready или обещать отсутствие всех ошибок. Отдельный privileged helper, подписанные manifests и расширенный compatibility matrix остаются roadmap, не заглушками существующего runtime.
