import { STATE_LABELS, appendLog, createRequestGate, errorMessage, parseTargets, validateBundle } from "./ui-state.js";

const $ = (id) => document.getElementById(id);
const desktop = typeof window.__TAURI__?.core?.invoke === "function";
let snapshot = null;
let lines = [];
let report = null;
let closing = false;
let bootCheckDone = false;

function log(message) {
  lines = appendLog(lines, message, new Date().toLocaleTimeString("ru-RU"));
  $("log").textContent = lines.join("\n");
  $("log").scrollTop = $("log").scrollHeight;
}
function controls(busy) {
  for (const button of document.querySelectorAll("[data-command]")) {
    const requiresAdmin = button.hasAttribute("data-admin");
    button.disabled = !desktop || busy || closing || (requiresAdmin && !snapshot?.administrator);
  }
  for (const input of document.querySelectorAll("input, select, textarea")) input.disabled = !desktop || busy || closing;
  document.body.setAttribute("aria-busy", String(busy));
}
const gate = createRequestGate(controls);
function show(value) {
  report = value;
  $("result").textContent = JSON.stringify(value, null, 2);
  $("download-report").disabled = false;
}
function notify(message, kind = "") {
  const node = $("feedback"); node.textContent = message; node.className = `notice ${kind}`; node.hidden = false;
}
function render(state) {
  snapshot = state;
  $("engine-version").textContent = state.bundle?.tag ?? "Не установлен";
  $("driver-state").textContent = state.driver_present ? "Файл найден" : "Не найден";
  $("admin-state").textContent = state.administrator ? "Администратор" : "Только чтение";
  $("root-path").textContent = state.root;
  $("service-state").textContent = state.service ? (STATE_LABELS[state.service.state] ?? "Неизвестно") : "Не установлена";
  $("service-dot").className = `status-dot ${state.service?.state === "running" ? "running" : ""}`;
  const selection = $("strategy").value;
  const options = state.strategies.map((name) => { const option = document.createElement("option"); option.value = name; option.textContent = name; return option; });
  if (!options.length) { const option = document.createElement("option"); option.value = ""; option.textContent = "Сначала импортируйте пакет"; options.push(option); }
  $("strategy").replaceChildren(...options);
  if (state.strategies.includes(selection)) $("strategy").value = selection;
  $("game").value = state.config.game_filter;
  $("ipset").value = state.config.ipset_filter;
  $("check-on-start").checked = state.config.check_updates_on_start;
  $("targets").value = state.config.targets.join("\n");
  const warnings = [];
  if (!state.administrator) warnings.push("Режим чтения. Для изменений запустите приложение от имени администратора.");
  if (!state.initialized) warnings.push("Подготовьте защищённый каталог перед первой установкой.");
  if (state.pending_recovery.length) warnings.push("Обнаружена прерванная операция. Используйте восстановление.");
  if (state.legacy_service_present) warnings.push("Обнаружена чужая служба zapret. Tandem не будет её удалять.");
  if (state.service_owned === false) warnings.push("Конфликт имени службы: изменения заблокированы проверкой владельца.");
  $("environment").textContent = warnings.join(" ") || "Локальный режим. Изменения только по вашему подтверждению; телеметрии нет.";
  controls(gate.busy);
}
async function invoke(action) {
  if (!desktop) throw new Error("Это просмотр интерфейса, не Windows-приложение. Системные операции недоступны.");
  return window.__TAURI__.core.invoke("request", { action });
}
async function refresh() { const state = await invoke({ type: "get_dashboard" }); render(state); return state; }
function confirmChange(message) {
  $("confirm-text").textContent = message;
  const dialog = $("confirm-dialog"); dialog.returnValue = "cancel";
  return new Promise((resolve) => {
    dialog.addEventListener("close", () => resolve(dialog.returnValue === "confirm"), { once: true });
    dialog.showModal();
  });
}
async function perform(label, action, confirmation = null, reload = false) {
  if (gate.busy || closing) return;
  try {
    if (confirmation && !(await confirmChange(confirmation))) return;
    await gate.run(async () => {
      $("feedback").hidden = true; log(`${label}: начало`);
      const result = await invoke(action); show(result); log(`${label}: выполнено`);
      if (reload) {
        try { await refresh(); } catch (error) { log(`Операция выполнена, но статус не обновлён: ${errorMessage(error)}`); notify("Операция завершена. Обновить статус не удалось — проверьте журнал.", "error"); return; }
      }
      notify(`${label}: выполнено.`, "success");
    });
  } catch (error) { const message = errorMessage(error); show({ ok: false, operation: label, error: message }); log(`${label}: ${message}`); notify(message, "error"); }
}
function bundleFields() { return validateBundle($("bundle-tag").value.trim(), $("bundle-sha").value.trim()); }
function requiredPath(id) { const path = $(id).value.trim(); if (!path) throw new Error("Укажите абсолютный путь к проверенному локальному файлу."); return path; }
function requiredStrategy() { const value = $("strategy").value; if (!value) throw new Error("Сначала импортируйте пакет и выберите стратегию."); return value; }
function bind(id, handler) {
  $(id).addEventListener("click", async () => { try { await handler(); } catch (error) { notify(errorMessage(error), "error"); log(errorMessage(error)); } });
}
bind("refresh", () => gate.run(refresh).catch((error) => { notify(errorMessage(error), "error"); }));
bind("initialize", () => perform("Подготовка каталога", { type: "initialize" }, "Создать защищённый каталог в Program Files и сохранить безопасную конфигурацию? Службы и hosts пока не изменяются.", true));
bind("import-bundle", () => perform("Импорт пакета", { type: "import_bundle", path: requiredPath("bundle-path"), ...bundleFields() }, "Вы подтверждаете, что SHA-256 получена из доверенного независимого источника? Пакет будет проверен и заменён с сохранением предыдущей версии. Служба должна быть удалена заранее.", true));
bind("download-bundle", () => perform("Загрузка пакета", { type: "download_bundle", ...bundleFields() }, "Скачать указанный релиз Flowseal с GitHub и сверить его с доверенной SHA-256? Изменение пакета возможно только при удалённой службе Tandem.", true));
bind("check-updates", () => perform("Сверка релиза", { type: "check_updates" }));
bind("rollback-bundle", () => perform("Откат пакета", { type: "rollback_bundle" }, "Вернуть предыдущий управляемый пакет? Сначала удалите службу Tandem. После отката её нужно установить заново.", true));
bind("preview", () => perform("План стратегии", { type: "preview_strategy", strategy: requiredStrategy() }));
bind("install", () => perform("Установка службы", { type: "install_service", strategy: requiredStrategy() }, "Установить выбранную стратегию как службу LocalSystem с автозапуском? Это изменит обработку сетевого трафика. Приложение проверит план, права и состояние службы.", true));
bind("start", () => perform("Запуск службы", { type: "start_service" }, "Запустить службу Tandem с ранее установленной стратегией?", true));
bind("stop", () => perform("Остановка службы", { type: "stop_service" }, "Остановить только службу Tandem? Связь с некоторыми сервисами может измениться.", true));
bind("remove", () => perform("Удаление службы", { type: "remove_service" }, "Остановить и удалить только tandem-zapret? Общие драйверы WinDivert, чужие службы и hosts не будут затронуты.", true));
bind("recover", () => perform("Восстановление", { type: "recover" }, "Восстановить согласованное состояние по журналам прерванных операций? Для hosts предусмотрена отдельная команда восстановления.", true));
bind("save-settings", () => {
  if (!snapshot) throw new Error("Сначала получите текущую конфигурацию.");
  const config = { ...snapshot.config, game_filter: $("game").value, ipset_filter: $("ipset").value, check_updates_on_start: $("check-on-start").checked, targets: parseTargets($("targets").value) };
  return perform("Сохранение настроек", { type: "save_config", config }, "Сохранить настройки и применить режим IPSet? Служба должна быть остановлена. Новые параметры стратегии применятся после её переустановки.", true);
});
bind("diagnostics", () => perform("Диагностика", { type: "diagnostics" }));
bind("test-targets", () => perform("Проверка доступности", { type: "test_targets" }));
bind("support", () => perform("Обезличенный отчёт", { type: "export_support" }));
bind("import-ipset", () => perform("Импорт IPSet", { type: "import_ipset", path: requiredPath("list-path") }, "Заменить исходный IPSet проверенными IP/CIDR из локального файла? Активный режим сохранится. Служба должна быть остановлена.", true));
bind("apply-hosts", () => perform("Применение hosts", { type: "apply_hosts", path: requiredPath("list-path") }, "Изменить системный hosts? Будут разрешены только домены из конфигурации и публичные IP. Оригинал сохранится для восстановления.", true));
bind("restore-hosts", () => perform("Восстановление hosts", { type: "restore_hosts" }, "Восстановить исходный hosts? Если файл был изменён другим приложением, автоматическая перезапись будет запрещена.", true));
bind("clear-log", () => { lines = []; $("log").textContent = ""; });
bind("download-report", () => {
  if (report === null) return;
  const file = new Blob([JSON.stringify(report, null, 2)], { type: "application/json" });
  const url = URL.createObjectURL(file); const link = document.createElement("a");
  link.href = url; link.download = "tandem-result.json"; link.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
});

async function boot() {
  if (!desktop) {
    $("environment").textContent = "Просмотр интерфейса. Это не работающий VPN и не Windows-клиент: системные действия отключены. Запустите desktop-сборку для реальных операций.";
    log("Открыт безопасный browser preview: backend отсутствует, результаты не симулируются."); controls(false); return;
  }
  if (window.__TAURI__?.event?.listen) {
    try {
      await window.__TAURI__.event.listen("shutdown-wait", (event) => { closing = true; controls(true); notify(String(event.payload)); });
    } catch (error) { log(`Не удалось подписаться на событие закрытия: ${errorMessage(error)}`); }
  }
  try {
    const state = await gate.run(refresh); log("Подключено локальное Windows-приложение.");
    if (state.config.check_updates_on_start && !bootCheckDone) { bootCheckDone = true; await perform("Проверка релиза при запуске", { type: "check_updates" }); }
  } catch (error) { notify(errorMessage(error), "error"); log(`Не удалось получить состояние: ${errorMessage(error)}`); }
}
await boot();
