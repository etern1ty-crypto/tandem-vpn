# Развёртывание и production gate

## Статус

Эта поставка — source-only release candidate. Windows installer и native executable не собраны в среде ревизии. Нельзя считать добавленный CI workflow успешно выполненным до его реального запуска. Проверки содержатся в [VERIFICATION.md](VERIFICATION.md).

## Требования

- Целевая runtime-платформа: Windows 11 x64. Windows ARM64, Windows 7, Wine и macOS/Linux как runtime для системных действий не поддерживаются.
- Rust stable; базовый код требует как минимум Rust 1.89 (file locking API). Верхний фактический MSRV транзитивных зависимостей проверяется компилятором, не предполагается по одному workspace-полю.
- Visual Studio Build Tools: workload Desktop development with C++, Windows SDK.
- WebView2 runtime.
- Node.js 22+; frontend не требует установки npm packages.
- Cargo/Tauri CLI 2; интернет нужен для первоначального получения Rust toolchain, crates и builder components.

## Сборка

Из корня проекта:

```powershell
cargo install tauri-cli --version "^2" --locked
cargo fmt --all
cargo test --locked -p tandem-core --all-targets
cargo build --locked --release -p tandem-core --bin tandem
cd app
npm ci --ignore-scripts --no-audit
npm run check
npm test
npm run build
cargo tauri build
```

CLI получится в `target\release\tandem.exe`, NSIS bundle — в `target\release\bundle\nsis`. Пути предполагают стандартный workspace target directory. Дополнительные Rust target flags могут изменить расположение.

### Lock-файлы

- `app/package-lock.json` содержит ноль внешних packages и соответствует manifest.
- `Cargo.lock` сохранён на версиях/registry checksums исходного архива, обновлён для новых direct dependencies и удаления shell/zip-extract. Граф проверен статически; настоящая Cargo resolution не была выполнена из-за отсутствия Cargo/сети.
- Первый online gate — `cargo metadata --locked` и `cargo test --locked`. Если Cargo требует канонизацию lock, выполните `cargo generate-lockfile`, рассмотрите diff и advisory scan, затем зафиксируйте результат. Не обходите `--locked` в публикации и не объявляйте статическую проверку equivalent реальной сборке.
- Используемые toolchain/actions тоже нужно закрепить по проверенным release/SHA в вашем production fork. `stable` в source candidate не обещает bit-for-bit воспроизводимость через год.

## Первая установка на разрешённой станции

1. Соберите, проверьте и установите GUI. Installer настроен `perMachine`; это не означает, что каждый последующий запуск приложения автоматически elevated.
2. Запустите приложение/CLI от имени администратора и выполните `init`.
3. Получите ZIP только из утверждённого источника. Сверьте digest с независимым каналом, вашим internal artifact review или подписанным внешним manifest. Получить hash через `Get-FileHash` полезно для сравнения, но само по себе не делает неизвестный файл доверенным.
4. Импортируйте конкретный bundle, просмотрите strategy plan, подтвердите установку службы.
5. Выполните диагностику и тест нужных сервисов. Отдельно проверьте голос, видео, QUIC и приложения: обычный HTTPS GET их не тестирует.
6. Перезагрузите Windows и проверьте автозапуск и процедуру остановки/удаления.

Не запускайте чужой `.bat` для подготовки каталогов и не отключайте антивирус. При детекте остановите развертывание, проверьте происхождение/подписи/суммы и обратитесь к политике безопасности организации.

## Миграция с исходного tandem-vpn

Старый каталог рядом с EXE, службу `zapret`, registry strategy metadata и marker-файлы новая версия автоматически не присваивает и не переносит. Сначала сохраните необходимые настройки и изучите, кто владеет старой установкой. Не используйте старое массовое удаление служб, если общий WinDivert нужен другому приложению. После согласованного удаления старой службы импортируйте утверждённый пакет в новый protected root и примените новую конфигурацию.

Hosts recovery point означает состояние **до операции этой версии**, а не factory-default Windows hosts и не гарантированную отмену исторических правок старого клиента.

## Обновление и откат

Для смены пакета требуется **удалённая**, а не просто остановленная Tandem-service. Это сознательная защита от несовместимых файлов и от изменения running executable. Обновление не обещает zero downtime.

```powershell
.\tandem.exe service remove
.\tandem.exe bundle import "C:\Approved\release.zip" $ApprovedSha256 $ApprovedTag
.\tandem.exe preview "general.bat"
.\tandem.exe service install "general.bat"
```

`$ApprovedSha256` и `$ApprovedTag` должны быть настоящими проверенными значениями; приложение не принимает фиктивную или пустую сумму. Для отката:

```powershell
.\tandem.exe service remove
.\tandem.exe bundle rollback
.\tandem.exe service install "general.bat"
```

Предыдущий пакет — один слот rollback, не бесконечная история. При импорте следующего пакета более старый slot удаляется. Пользовательские списки и game configuration нужно проверить после смены версии; upstream layout может измениться.

## Удаление

1. Остановите/удалите **только свою** службу: `tandem service remove`.
2. При наличии recovery point выполните `tandem hosts restore`; конфликт внешнего изменения разберите вручную.
3. Сохраните нужный support report и необходимые журналы по политике организации.
4. Удалите GUI через Windows Apps. Приложение не очищает автоматически общий WinDivert и не удаляет unmanaged installations.
5. После проверки отсутствия зависимости от данных удалите `Program Files\TandemWorkbench` административно. Не переносите исполняемые файлы службы в user-writable directory.

## CI и выпуск

`ci.yml` выполняет frontend checks, Linux core tests, Windows checks/tests, форматирование в CI и advisory gate. Formatting output и Cargo logs должны быть рассмотрены; workflow-файл сам по себе не доказывает прохождение. `release.yml` создаёт NSIS/CLI assets только после своего полного Windows test/audit gate, для тега, соответствующего версии. GitHub release создаётся **draft**, не автоматически опубликованным.

Отдельные необходимые production gates:

- `cargo fmt`, компиляция, Clippy, core/CLI tests и Windows smoke — зелёные.
- Cargo advisory scan по актуальной базе, без замалчивания findings.
- Проверка default ACL/owner, inherited ACL, restricted token, denied access и non-ASCII install paths.
- Настоящий Flowseal ZIP: файл/драйвер, imports, register/start/stop/remove, reboot, bundle rollback, emergency recovery.
- Подпись установщика и reviewed provenance upstream binaries; signing key не предоставлялся и не придуман.
- Подтверждённая legal/policy eligibility каждой deployment group.
- Согласование отделения privileged helper до масштабного многопользовательского внедрения.

Пока эти пункты не выполнены, не публикуйте артефакты как проверенный безопасный production-клиент.
