// tandem-vpn frontend (vanilla). Talks to the Rust backend via Tauri's
// `invoke`. Falls back to a no-op shim when opened in a plain browser so the
// UI can be inspected without the desktop shell.

const LOCAL_ZAPRET_VERSION = "1.9.9a";

const tauri = window.__TAURI__;
const invoke = tauri?.core?.invoke
  ? (cmd, args) => tauri.core.invoke(cmd, args)
  : async (cmd) => {
      log(`[browser] invoke('${cmd}') недоступен вне Tauri`);
      throw new Error("Tauri API недоступен (открыто в браузере)");
    };
const openUrl = (url) =>
  tauri?.shell?.open ? tauri.shell.open(url) : window.open(url, "_blank");

const $ = (id) => document.getElementById(id);
const logEl = () => $("log");

function log(msg) {
  const ts = new Date().toLocaleTimeString();
  const el = logEl();
  el.textContent += `[${ts}] ${msg}\n`;
  el.scrollTop = el.scrollHeight;
}

function badge(el, text, kind) {
  el.textContent = text;
  el.className = `val ${kind || ""}`;
}

const STATE_LABELS = {
  running: ["RUNNING", "ok"],
  stopped: ["STOPPED", "warn"],
  stop_pending: ["STOP_PENDING", "warn"],
  start_pending: ["START_PENDING", "warn"],
  not_installed: ["не установлено", "muted"],
  unknown: ["неизвестно", "muted"],
};

async function refreshStatus() {
  try {
    const s = await invoke("get_status");
    const [svc, svcKind] = STATE_LABELS[s.service] || [s.service, ""];
    const [wd, wdKind] = STATE_LABELS[s.windivert] || [s.windivert, ""];
    badge($("st-service"), svc, svcKind);
    badge($("st-windivert"), wd, wdKind);
    badge($("st-winws"), s.winws_running ? "запущен" : "не запущен", s.winws_running ? "ok" : "warn");
    badge($("st-sys"), s.windivert_sys_present ? "найден" : "нет", s.windivert_sys_present ? "ok" : "warn");
    badge($("st-strategy"), s.installed_strategy || "—", "muted");
  } catch (e) {
    log(`Статус: ${e}`);
  }
}

async function loadStrategies() {
  try {
    const list = await invoke("list_strategies");
    const sel = $("strategy");
    sel.innerHTML = "";
    if (!list.length) {
      const opt = document.createElement("option");
      opt.textContent = "стратегии не найдены — укажите папку zapret";
      opt.disabled = true;
      sel.appendChild(opt);
      return;
    }
    for (const name of list) {
      const opt = document.createElement("option");
      opt.value = name;
      opt.textContent = name;
      sel.appendChild(opt);
    }
  } catch (e) {
    log(`Стратегии: ${e}`);
  }
}

async function loadSettings() {
  try {
    const s = await invoke("get_settings");
    $("game").checked = s.game_filter;
    $("autoupd").checked = s.auto_update;
    $("ipset").value = s.ipset_filter;
    log(`Папка установки: ${s.install_dir}`);
  } catch (e) {
    log(`Настройки: ${e}`);
  }
}

function wire() {
  document.querySelectorAll(".tab").forEach((t) =>
    t.addEventListener("click", () => {
      if (t.disabled) return;
      document.querySelectorAll(".tab").forEach((x) => x.classList.remove("active"));
      t.classList.add("active");
    })
  );

  $("refresh").onclick = refreshStatus;
  $("clear-log").onclick = () => (logEl().textContent = "");

  $("install").onclick = async () => {
    const strategy = $("strategy").value;
    if (!strategy) return log("Выберите стратегию");
    log(`Установка службы со стратегией «${strategy}»…`);
    try {
      await invoke("install_service", { strategy, gameFilter: $("game").checked });
      log("Служба установлена и запущена.");
      refreshStatus();
    } catch (e) {
      log(`Ошибка установки: ${e}`);
    }
  };

  $("remove").onclick = async () => {
    log("Удаление служб (zapret + WinDivert)…");
    try {
      await invoke("remove_service");
      log("Службы удалены.");
      refreshStatus();
    } catch (e) {
      log(`Ошибка удаления: ${e}`);
    }
  };

  $("game").onchange = (e) =>
    invoke("set_game_filter", { enabled: e.target.checked })
      .then(() => log(`Game Filter: ${e.target.checked ? "вкл" : "выкл"} (применится при установке)`))
      .catch((err) => log(`Game Filter: ${err}`));

  $("autoupd").onchange = (e) =>
    invoke("set_auto_update", { enabled: e.target.checked })
      .then(() => log(`Auto-Update: ${e.target.checked ? "вкл" : "выкл"}`))
      .catch((err) => log(`Auto-Update: ${err}`));

  $("ipset").onchange = (e) =>
    invoke("set_ipset_filter", { mode: e.target.value })
      .then(() => log(`IPSet Filter: ${e.target.value}`))
      .catch((err) => log(`IPSet Filter: ${err}`));

  $("upd-ipset").onclick = async () => {
    log("Загрузка актуального ipset-all.txt…");
    try {
      const n = await invoke("update_ipset_list");
      log(`IPSet-список обновлён: ${n} записей.`);
    } catch (e) {
      log(`Обновление IPSet: ${e}`);
    }
  };

  $("chk-upd").onclick = async () => {
    log("Проверка обновлений zapret…");
    try {
      const r = await invoke("check_updates", { localVersion: LOCAL_ZAPRET_VERSION });
      if (r.update_available) {
        log(`Доступна новая версия: ${r.remote} (локальная ${r.local}).`);
        openUrl(r.release_url);
      } else {
        log(`Установлена последняя версия: ${r.local}.`);
      }
    } catch (e) {
      log(`Проверка обновлений: ${e}`);
    }
  };

  $("diag").onclick = async () => {
    log("Диагностика…");
    try {
      const d = await invoke("run_diagnostics");
      log(`BFE: ${d.bfe_running ? "OK" : "НЕ запущен"}; .sys: ${d.windivert_sys_present ? "есть" : "нет"}; winws: ${d.winws_running ? "запущен" : "нет"}`);
      d.notes.forEach((n) => log(`  • ${n}`));
      if (!d.notes.length) log("Проблем не обнаружено.");
    } catch (e) {
      log(`Диагностика: ${e}`);
    }
  };

  $("tests").onclick = async () => {
    log("Тест доступности целей…");
    try {
      const res = await invoke("run_tests");
      res.forEach((r) =>
        log(`  ${r.ok ? "✓" : "✗"} ${r.url} — ${r.status ?? r.error ?? ""} (${r.ms} ms)`)
      );
    } catch (e) {
      log(`Тесты: ${e}`);
    }
  };
}

window.addEventListener("DOMContentLoaded", () => {
  wire();
  log("tandem-vpn запущен. Фаза 1 — Zapret.");
  loadSettings();
  loadStrategies();
  refreshStatus();
});
