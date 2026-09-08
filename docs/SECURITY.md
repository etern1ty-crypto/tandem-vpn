# Модель угроз и пределы гарантий

## Защищаемые ресурсы

Привилегированная Windows-служба, исполняемые файлы движка, системный hosts, локальные настройки, корректность выводов диагностики, конфиденциальность содержимого support report. Намеренно не заявляется защита от уже скомпрометированного SYSTEM/Administrator или вредоносного kernel driver.

## Границы доверия

1. GUI-поля, имена стратегий, downloaded bytes и содержимое ZIP — untrusted до соответствующей валидации.
2. Пользовательское подтверждение SHA-256 должно опираться на реальный независимый источник. HTTPS и SHA защищают транспорт/целостность, но не делают злонамеренный upstream доброкачественным.
3. Защищённый root должен быть недоступен для записи обычному пользователю. Проверяются owner, DACL и reparse points; служба не исполняется из AppData/current directory.
4. `.bat` не исполняется. Парсер отдаёт только approved option subset в фиксированный protected `winws.exe`.
5. `sc.exe` берётся из native System32 known directory; `cmd.exe`, PowerShell, PATH lookup и arbitrary program names не принимаются от UI.
6. Service ownership проверяется по ожидаемому executable prefix, LocalSystem account и поддерживаемому start type. Это защита от непреднамеренного управления чужой службой, не attestation против администратора, который может изменить всё.

## Ограничения ZIP

ZIP до 64 MiB, не более 2 048 записей, до 256 MiB expanded и 64 MiB на файл. Запрещены absolute/traversal paths, backslashes в archive names, Windows device filenames, ADS/colon, case collisions и Unix special/link entries. Учитывается реальное количество распакованных bytes. Чтение проходит через ZIP decompressor с CRC validation, запись — только в новый staging. Высокоуровневый `ZipArchive::extract` не вызывается.

Проверка `MZ`/минимального размера PE — structural sanity check, не Authenticode verification и не malware scan. Подпись/происхождение binary и driver необходимо проверять отдельно перед production.

## Сеть и данные

- Только явные HTTPS-запросы к GitHub для версии/пакета и к конфигурированным target URLs для диагностики.
- Жёсткая allowlist download origins, максимум шесть попыток в redirect loop; TLS verification не отключается.
- Public-only DNS resolution используется при соединении, не только до него. Private/mixed answers отклоняются; редиректы connectivity probes не выполняются.
- Не делаем никаких network requests при обычном browser preview. Opt-in check-on-start по умолчанию выключен.
- Нет аналитического SDK, фонового upload, аккаунтов и встроенных API keys.
- Журнал UI ограничен 200 строками; системный audit ротируется после 1 MiB. Ошибки обрезаются до 2 048 символов. Raw logs могут содержать локальные пути/имена, поэтому support report их не включает.

## Recovery не равно абсолютная атомарность

Journals снижают риск partial changes и предоставляют повторяемую процедуру восстановления. Они не защищают от аппаратной порчи, concurrent writes другого привилегированного продукта или вредоносного администратора. Hosts compare-before-replace не является межприложенческим atomic CAS. Windows tests должны включать принудительное завершение процесса в разных фазах.

## Зависимости

Исходный npm lock не соответствовал package.json. Frontend теперь использует ноль внешних npm packages; Vite/esbuild и неиспользуемые Tauri JS dependencies удалены вместе с их attack surface. Rust `tauri-plugin-shell` и `zip-extract` удалены; остальные registry versions/checksums сохранены из исходного lock без объявления их проверенными по актуальной advisory database.

Изучен [CVE-2025-29787 / GHSA-94vh-gphv-8pm8](https://github.com/advisories/GHSA-94vh-gphv-8pm8): опубликованный affected range указан как `>=1.3.0,<2.3.0`. Поэтому некорректно объявлять исходный `zip 0.6.6` доказанно уязвимым именно по этой записи. Кроме того, переработка не вызывает описанные high-level extraction methods. Это **не заменяет актуальный cargo-audit по всему графу**. Более новые записи и unmaintained dependencies необходимо проверять перед выпуском.

## Что остаётся обязательным до production

Независимый review unsafe Win32 boundary, реальный SCM/ACL/reboot acceptance, свежий advisory scan, code signing и reviewed engine manifests. Elevated GUI остаётся техническим долгом: следующий этап — отдельный минимальный privileged helper с IPC policy. Поддержка всех будущих upstream strategies не заявляется.

При детекте AV не добавляйте blanket exclusions и не отключайте защиту. Остановите deployment и разберите причину по политике организации.
