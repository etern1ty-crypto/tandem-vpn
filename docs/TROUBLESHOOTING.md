# Восстановление и диагностика

## Перед повторным запуском операции

Если результат неясен, сначала обновите status и проверьте `pending_recovery`. Не удаляйте journals «чтобы разблокировать кнопки»: это может уничтожить единственную информацию о предыдущем состоянии. Busy и interrupted operation — разные состояния.

## Типичные случаи

| Симптом | Что проверить / делать |
|---|---|
| Browser preview, кнопки disabled | Это ожидаемо: Node preview не имеет Windows backend. Соберите desktop/CLI |
| Administrator required | Запустите только доверенную сборку elevated; чтение статуса оставьте обычному процессу |
| Non-administrator write permission / untrusted owner | Не ослабляйте проверку. Исследуйте ACL root и всех children, reparse points и происхождение каталога |
| Another operation active | Дождитесь текущего процесса. File lock автоматически снимается ОС при его завершении |
| Recovery journal exists | Выполните `tandem recover`; hosts имеет отдельный restore |
| Legacy zapret service exists | Определите владельца старой установки и удалите её её же инструментом, если это разрешено |
| BFE not running | Изучите Windows Services и политики устройства; Workbench не меняет BFE глобально |
| Strategy unsupported | Посмотрите точный unknown flag/placeholder. Поддерживается ограниченный parser, не cmd interpreter |
| Missing reference / ACTIVE packet file | Пакет несовместим/неполон или AV удалил файл. Не создавайте фиктивную пустую PE/packet заглушку |
| ZIP checksum mismatch | Не выполняйте пакет; проверьте утверждённый tag/digest и источник |
| Нельзя импортировать пакет | Для смены engine требуется удалённая Tandem-service, не только stopped |
| Настройки сохранены, game mode не изменился | После смены аргументов нужно `service install STRATEGY`, а не только start |
| HTTP 403 | TLS/HTTP ответ получен. Это может быть политика сайта/auth; не равнозначно сетевому timeout |
| Private/mixed DNS answer forbidden | Включилась SSRF/public-address policy. Внутренние цели намеренно не поддерживаются |
| GitHub asset отсутствует/несколько | Не выбирайте первый случайный ZIP. Импортируйте рассмотренный локальный пакет вручную |
| Окно ждёт при закрытии | Выполняется bounded native/network операция; окно не прерывает критическую запись |

## Recovery

```powershell
.\tandem.exe status
.\tandem.exe recover
.\tandem.exe doctor
```

Восстановление SCM может само получить access denied или timeout. В таком случае journal остаётся, а ошибка сообщает и исходную проблему, и проблему rollback. Исправьте причину в Windows, затем повторите recover. Если directory layout неоднозначен, остановитесь и сохраните копию journal/данных для ручного исследования.

Для hosts:

```powershell
.\tandem.exe hosts restore
```

Restore сравнивает hash текущего файла с ожидаемым applied/original hash. При внешних правках отказ намеренный: полная перезапись затёрла бы изменения другого инструмента. Оригинал хранится в `hosts-recovery.json`; сравните его с текущим hosts вручную, сохранив все необходимые mappings. Не публикуйте hosts или такой journal в публичных issues.

## Автозапуск и остановка

Успешная установка службы запрашивает `start=auto`, затем подтверждает `Running`. Закрытие GUI не останавливает службу. Отдельно проверьте реальный reboot и сценарий удаления на целевой Windows-сборке. Workbench не обещает совместимость с каждым anti-cheat, другим DPI-инструментом, VPN или NAT/RAS конфигурацией.

## Глобальные TCP timestamps

Исходник безусловно менял global TCP timestamps и не восстанавливал значение. В новой версии эта side effect удалена. Если одобренная стратегия требует другой настройки TCP, исследуйте и согласуйте её отдельно. Автоматически переписывать глобальную сеть ради любого нового upstream preset небезопасно.

## Что приложить к issue

Версию Workbench, target Windows version, описание ожидаемого поведения и результат `tandem support`. Даже обезличенный отчёт просмотрите перед публикацией. Не отправляйте secrets, полный hosts, raw event logs, private target URLs и corporate paths. Скриншот с `TEST-FIXTURE` относится к UI-тесту и не является доказательством состояния Windows.
