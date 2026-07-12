// tandem-vpn frontend (vanilla). Talks to the Rust backend via Tauri's
// `invoke`. Falls back to a no-op shim when opened in a plain browser so the
// UI can be inspected without the desktop shell.

const tauri = window.__TAURI__;
const invoke = tauri?.core?.invoke
  ? (cmd, args) => tauri.core.invoke(cmd, args)
  : async (cmd) => {
      log(`[browser] invoke('${cmd}') недоступен вне Tauri`);
      throw new Error("Tauri API недоступен (открыто в браузере)");
    };

const $ = (id) => document.getElementById(id);
const logEl = () => $("log");

function log(msg) {
  const ts = new Date().toLocaleTimeString();
  const el = logEl();
  if (!el) return;
  el.textContent += `[${ts}] ${msg}\n`;
  el.scrollTop = el.scrollHeight;
}

function badge(el, text, kind) {
  if (!el) return;
  el.textContent = text;
  el.className = `val ${kind || ""}`;
}

function openExternal(url) {
  if (tauri?.shell?.open) {
    tauri.shell.open(url);
  } else {
    window.open(url, "_blank");
  }
  log(`Открыт сайт: ${url}`);
}

// For the Tests (Dev) tab inline buttons
window.openTestSite = openExternal;

/** Disable a button and show a transient label while an async action runs. */
async function withBusy(btn, busyLabel, fn) {
  if (!btn) return fn();
  const prev = btn.textContent;
  btn.disabled = true;
  if (busyLabel) btn.textContent = busyLabel;
  try {
    return await fn();
  } finally {
    btn.disabled = false;
    btn.textContent = prev;
  }
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c])
  );
}

// ---------------------------------------------------------------- Engine

const STATE_LABELS = {
  running: ["RUNNING", "ok"],
  stopped: ["STOPPED", "warn"],
  stop_pending: ["STOP_PENDING", "warn"],
  start_pending: ["START_PENDING", "warn"],
  not_installed: ["не установлено", "muted"],
  unknown: ["неизвестно", "muted"],
};

let activeGoidaTag = null;

async function refreshEngine() {
  try {
    const s = await invoke("get_engine_status");
    const [svc, svcKind] = STATE_LABELS[s.service] || [s.service, ""];
    badge($("st-service"), svc, svcKind);
    badge($("st-singbox"), s.sing_box_running ? "запущен" : "не запущен", s.sing_box_running ? "ok" : "warn");
    badge($("st-binary"), s.binary_present ? "найден" : "нет", s.binary_present ? "ok" : "warn");
  } catch (e) {
    log(`Статус движка: ${e}`);
  }
}

async function loadSettings() {
  try {
    const s = await invoke("get_settings");
    const el = $("install-dir");
    if (el) el.textContent = s.install_dir;
  } catch (e) {
    log(`Настройки: ${e}`);
  }
}

// ------------------------------------------------------------------- WARP

const YESNO = (ok) => (ok ? ["готово", "ok"] : ["нет", "warn"]);

async function refreshWarp() {
  try {
    const w = await invoke("warp_status");
    badge($("wp-wgcf"), ...YESNO(w.wgcf_present));
    badge($("wp-account"), ...YESNO(w.registered));
    badge($("wp-profile"), ...YESNO(w.profile_generated));
  } catch (e) {
    log(`Статус WARP: ${e}`);
  }
}

// ------------------------------------------------------------------ Goida

const best = (items) => Math.min(...items.map((i) => i.delay_ms));
const delayKind = (ms) => (ms < 80 ? "ok" : ms < 140 ? "warn" : "muted");

/** Render tested Goida servers grouped by country, best (lowest) delay first. */
function renderGoidaResults(results) {
  const card = $("goida-results-card");
  const wrap = $("goida-results");
  wrap.innerHTML = "";
  if (!results.length) {
    card.hidden = false;
    wrap.innerHTML = `<p class="hint">Ни один сервер не прошёл тест (все с задержкой ≥ 200 мс или недоступны).</p>`;
    return;
  }

  // Group by country (flag + code); unknown countries bucketed last.
  const groups = new Map();
  for (const r of results) {
    const key = r.country_code || "??";
    if (!groups.has(key)) groups.set(key, { flag: r.country_flag || "🏳️", code: key, items: [] });
    groups.get(key).items.push(r);
  }
  // Sort groups by their best (lowest) delay.
  const sorted = [...groups.values()].sort((a, b) => best(a.items) - best(b.items));

  for (const g of sorted) {
    g.items.sort((a, b) => a.delay_ms - b.delay_ms);
    const block = document.createElement("div");
    block.className = "country-block";
    block.innerHTML = `<div class="country-head"><span class="flag">${g.flag}</span>
      <span class="country-code">${escapeHtml(g.code)}</span>
      <span class="muted">${g.items.length} серв.</span></div>`;
    for (const r of g.items) {
      const row = document.createElement("button");
      row.className = "server-row";
      row.dataset.tag = r.tag;
      if (r.tag === activeGoidaTag) row.classList.add("active");
      row.innerHTML = `<span class="server-name">${escapeHtml(r.remark || r.tag)}</span>
        <span class="delay ${delayKind(r.delay_ms)}">${r.delay_ms} мс</span>`;
      row.onclick = () => selectGoida(r.tag, r.remark);
      block.appendChild(row);
    }
    wrap.appendChild(block);
  }
  card.hidden = false;
}

async function selectGoida(tag, remark) {
  log(`Активация сервера «${remark || tag}»…`);
  try {
    await invoke("goida_select", { tag });
    activeGoidaTag = tag;
    badge($("st-active"), remark || tag, "ok");
    log("Сервер активирован, движок перезапущен с новой конфигурацией.");
    document.querySelectorAll(".server-row").forEach((el) =>
      el.classList.toggle("active", el.dataset.tag === activeGoidaTag)
    );
    refreshEngine();
  } catch (e) {
    log(`Ошибка активации сервера: ${e}`);
  }
}

// ------------------------------------------------------------------ Rules

const BUCKET_KIND = { direct: "muted", warp: "ok", goida: "warn" };

function renderOverrides(list) {
  const wrap = $("override-list");
  wrap.innerHTML = "";
  if (!list.length) {
    wrap.innerHTML = `<p class="hint">Персональных правил пока нет.</p>`;
    return;
  }
  for (const o of list) {
    const row = document.createElement("div");
    row.className = "override-row";
    row.innerHTML = `<span class="ov-domain">${escapeHtml(o.domain)}</span>
      <span class="pill ${BUCKET_KIND[o.bucket] || ""}">${o.bucket}</span>`;
    const del = document.createElement("button");
    del.className = "ghost small";
    del.textContent = "Удалить";
    del.onclick = () => removeOverride(o.domain);
    row.appendChild(del);
    wrap.appendChild(row);
  }
}

async function loadOverrides() {
  try {
    renderOverrides(await invoke("rules_get_overrides"));
  } catch (e) {
    log(`Правила: ${e}`);
  }
}

async function removeOverride(domain) {
  try {
    renderOverrides(await invoke("rules_remove_override", { domain }));
    log(`Правило для «${domain}» удалено.`);
  } catch (e) {
    log(`Ошибка удаления правила: ${e}`);
  }
}

// -------------------------------------------------------------- Wiring UI

function wireTabs() {
  document.querySelectorAll(".tab").forEach((t) =>
    t.addEventListener("click", () => {
      document.querySelectorAll(".tab").forEach((x) => x.classList.remove("active"));
      document.querySelectorAll(".panel").forEach((p) => p.classList.remove("active"));
      t.classList.add("active");
      $(t.dataset.tab)?.classList.add("active");
    })
  );
}

function wire() {
  wireTabs();
  $("clear-log").onclick = () => (logEl().textContent = "");

  // ------ Движок
  $("engine-refresh").onclick = refreshEngine;

  $("engine-download").onclick = (e) =>
    withBusy(e.currentTarget, "Скачивание…", async () => {
      log("Скачивание последнего релиза sing-box… это может занять минуту.");
      try {
        await invoke("download_singbox_release");
        log("sing-box скачан и распакован.");
        refreshEngine();
        // Extra refreshes in case extraction or status takes a moment
        setTimeout(() => refreshEngine(), 800);
        setTimeout(() => refreshEngine(), 2000);
      } catch (err) {
        log(`Ошибка скачивания sing-box: ${err}`);
      }
    });

  $("engine-start").onclick = (e) =>
    withBusy(e.currentTarget, "Запуск…", async () => {
      log("Установка и запуск службы движка (нужны права администратора)…");
      try {
        // Fire the install. It can take several seconds (schtasks create + run).
        // We do NOT block the UI forever — poll status a few times.
        invoke("install_engine")
          .then(() => log("Команда запуска службы отправлена."))
          .catch((err) => log(`Ошибка запуска движка: ${err}`));

        // Give it time and poll status so UI stays responsive
        setTimeout(() => refreshEngine(), 1200);
        setTimeout(() => refreshEngine(), 2800);
        setTimeout(() => refreshEngine(), 4500);
      } catch (err) {
        log(`Ошибка: ${err}`);
      }
    });

  $("engine-stop").onclick = (e) =>
    withBusy(e.currentTarget, "Остановка…", async () => {
      log("Остановка и удаление службы движка…");
      try {
        await invoke("remove_engine");
        activeGoidaTag = null;
        badge($("st-active"), "—", "muted");
        log("Движок остановлен.");
        refreshEngine();
      } catch (err) {
        log(`Ошибка остановки движка: ${err}`);
      }
    });

  // Admin elevation helper (shows UAC prompt and relaunches the app)
  const adminBtn = $("request-admin");
  if (adminBtn) {
    adminBtn.onclick = async () => {
      log("Запрашиваем перезапуск с правами администратора…");
      try {
        await invoke("relaunch_as_admin");
      } catch (e) {
        log(`Не удалось запросить повышение: ${e}. Попробуйте запустить .exe правой кнопкой → «Запуск от имени администратора».`);
      }
    };
  }

  $("engine-test").onclick = (e) =>
    withBusy(e.currentTarget, "Тест…", async () => {
      const wrap = $("target-results");
      wrap.innerHTML = `<p class="hint">Проверка целей…</p>`;
      try {
        const res = await invoke("run_tests");
        wrap.innerHTML = "";
        for (const r of res) {
          const row = document.createElement("div");
          row.className = "target-row";
          const status = r.ok ? "✓" : "✗";
          row.innerHTML = `<span class="${r.ok ? "ok" : "warn"}">${status}</span>
            <span class="target-url">${escapeHtml(r.url)}</span>
            <span class="muted">${escapeHtml(String(r.status ?? r.error ?? ""))} · ${r.ms} мс</span>`;
          wrap.appendChild(row);
        }
      } catch (err) {
        wrap.innerHTML = `<p class="hint">Ошибка теста: ${escapeHtml(String(err))}</p>`;
      }
    });

  // ------ WARP
  $("warp-refresh").onclick = refreshWarp;

  $("warp-download").onclick = (e) =>
    withBusy(e.currentTarget, "Скачивание…", async () => {
      log("Скачивание wgcf…");
      try {
        await invoke("download_wgcf_release");
        log("wgcf скачан.");
        refreshWarp();
      } catch (err) {
        log(`Ошибка скачивания wgcf: ${err}`);
      }
    });

  $("warp-register").onclick = (e) =>
    withBusy(e.currentTarget, "Регистрация…", async () => {
      log("Создание бесплатного WARP-аккаунта и генерация профиля…");
      try {
        await invoke("warp_register");
        log("WARP-аккаунт создан. Нажмите «Запустить движок», чтобы применить.");
        refreshWarp();
      } catch (err) {
        log(`Ошибка регистрации WARP: ${err}`);
      }
    });

  // ------ Goida
  $("goida-load").onclick = (e) =>
    withBusy(e.currentTarget, "Загрузка…", async () => {
      const url = $("goida-url").value.trim();
      if (!url) return log("Укажите ссылку на подписку.");
      log(`Загрузка списка конфигов: ${url}`);
      try {
        const configs = await invoke("goida_fetch_list", { url });
        $("goida-count").textContent = `${configs.length} конфигов загружено`;
        $("goida-count").className = "pill ok";
        $("goida-test").disabled = configs.length === 0;
        log(`Загружено ${configs.length} конфигов. Нажмите «Тестировать».`);
      } catch (err) {
        log(`Ошибка загрузки списка: ${err}`);
      }
    });

  $("goida-test").onclick = (e) =>
    withBusy(e.currentTarget, "Тестирование…", async () => {
      const prog = $("goida-progress");
      const fill = $("goida-progress-fill");
      const text = $("goida-progress-text");
      prog.classList.remove("hidden");
      fill.style.width = "10%";
      text.textContent = "Поднимаю движок в тест-режиме и пингую серверы… (интерфейсы временно отключены)";
      log("Тест Goida: движок переведён в тест-режим, идёт пинг серверов…");
      // Indeterminate creep so the bar visibly moves during the batch test.
      let pct = 10;
      const timer = setInterval(() => {
        pct = Math.min(pct + 3, 90);
        fill.style.width = `${pct}%`;
      }, 700);
      try {
        const results = await invoke("goida_test_all");
        clearInterval(timer);
        fill.style.width = "100%";
        text.textContent = `Готово: ${results.length} рабочих серверов (задержка < 200 мс).`;
        log(`Тест завершён: ${results.length} серверов прошли фильтр.`);
        renderGoidaResults(results);
        setTimeout(() => prog.classList.add("hidden"), 1500);
      } catch (err) {
        clearInterval(timer);
        prog.classList.add("hidden");
        log(`Ошибка теста Goida: ${err}`);
      }
    });

  // ------ Правила
  $("rules-refresh-community").onclick = (e) =>
    withBusy(e.currentTarget, "Обновление…", async () => {
      log("Обновление списка РФ-блокировок…");
      try {
        const n = await invoke("rules_refresh_community_list");
        log(`Список обновлён: ${n} доменов. Применится при следующем запуске движка.`);
      } catch (err) {
        log(`Ошибка обновления списка: ${err}`);
      }
    });

  $("override-add").onclick = async () => {
    const domain = $("override-domain").value.trim();
    const bucket = $("override-bucket").value;
    if (!domain) return log("Укажите домен.");
    try {
      renderOverrides(await invoke("rules_set_override", { domain, bucket }));
      $("override-domain").value = "";
      log(`Правило добавлено: ${domain} → ${bucket}. Применится при следующем запуске движка.`);
    } catch (e) {
      log(`Ошибка добавления правила: ${e}`);
    }
  };

  // ------ Тесты (Автоматизированная диагностика)
  async function dumpToLog(title, data) {
    log(`\n========== ${title} ==========`);
    if (typeof data === 'string') {
      log(data);
    } else {
      try {
        log(JSON.stringify(data, null, 2));
      } catch (e) {
        log(String(data));
      }
    }
    log(`========== /${title} ==========\n`);
  }

  const diagFull = $("diag-full");
  if (diagFull) {
    diagFull.onclick = async () => {
      log("Запуск ПОЛНОЙ ДИАГНОСТИКИ...");
      try {
        const report = await invoke("get_diagnostics_report");
        dumpToLog("FULL DIAGNOSTICS REPORT", report);
      } catch (e) {
        log(`Ошибка полной диагностики: ${e}`);
      }
      // Also run connectivity
      try {
        const conn = await invoke("run_tests");
        dumpToLog("CONNECTIVITY TESTS", conn);
      } catch (e) {}
    };
  }

  const diagEngine = $("diag-engine");
  if (diagEngine) {
    diagEngine.onclick = async () => {
      log("Диагностика Движка...");
      try {
        const status = await invoke("get_engine_status");
        dumpToLog("ENGINE STATUS", status);
        const report = await invoke("get_diagnostics_report");
        // Extract relevant part or just log full for now
        log("--- Engine related from full report (см. выше если был) ---");
      } catch (e) { log(`Err: ${e}`); }
    };
  }

  const diagWarp = $("diag-warp");
  if (diagWarp) {
    diagWarp.onclick = async () => {
      log("Диагностика WARP...");
      try {
        const w = await invoke("warp_status");
        dumpToLog("WARP STATUS", w);
        const report = await invoke("get_diagnostics_report");
        dumpToLog("WARP DETAILED (from report)", report);
      } catch (e) { log(`Err: ${e}`); }
    };
  }

  const diagGoida = $("diag-goida");
  if (diagGoida) {
    diagGoida.onclick = async () => {
      log("Диагностика Goida...");
      try {
        const report = await invoke("get_diagnostics_report");
        dumpToLog("GOIDA + GENERAL REPORT", report);
      } catch (e) { log(`Err: ${e}`); }
    };
  }

  const diagConn = $("diag-connect");
  if (diagConn) {
    diagConn.onclick = async () => {
      log("Тесты связности...");
      try {
        const res = await invoke("run_tests");
        dumpToLog("CONNECTIVITY", res);
      } catch (e) { log(`Err: ${e}`); }
    };
  }

  const adminCheckBtn = $("check-admin-btn");
  if (adminCheckBtn) {
    adminCheckBtn.onclick = async () => {
      try {
        const isAdmin = await invoke("check_admin");
        const el = $("admin-status");
        if (el) {
          el.textContent = isAdmin ? "АДМИН ✓" : "НЕ АДМИН ✗";
          el.className = isAdmin ? "pill ok" : "pill warn";
        }
        log(`Admin check: ${isAdmin}`);
      } catch (e) {
        log(`Admin check error: ${e}`);
      }
    };
  }

  // Run admin check on load for the tests tab
  setTimeout(async () => {
    try {
      const isAdmin = await invoke("check_admin");
      const el = $("admin-status");
      if (el) {
        el.textContent = isAdmin ? "АДМИН ✓" : "НЕ АДМИН ✗";
        el.className = isAdmin ? "pill ok" : "pill warn";
      }
    } catch (_) {}
  }, 800);
}

window.addEventListener("DOMContentLoaded", () => {
  wire();
  log("tandem-vpn запущен.");
  loadSettings();
  refreshEngine();
  refreshWarp();
  loadOverrides();
});
