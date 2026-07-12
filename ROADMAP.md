# tandem-vpn — MVP Roadmap

Turning the baseline into a real, demand-worthy MVP. I kept your five goals but **reordered by dependency**: logging comes first because you cannot fix WARP or tune the speedtest engine blind — every current pain ("engine crashes, 0 servers, WARP silently fails") is really *a visibility failure*. Ship eyes first, then hands.

Architecture invariant preserved throughout: **`core` stays offline/pure (parsing, rendering, scoring, ranking) with network I/O only in the Tauri layer.** Every new algorithm lands as a unit-tested pure function; the app orchestrates.

---

## Phase 1 — Logging & visibility foundation (the enabler)

**Why first:** without this, phases 2–3 are guesswork. Three layers:

### 1a. sing-box engine logs → file
- `core/src/engine/mod.rs::render_config`: add `"output": "<install_dir>/sing-box.log"` to the `log` block (currently `{level, timestamp}` → stdout, which is *lost* under the headless SYSTEM scheduled task — this is why the crashes were invisible).
- **Validation gate:** `sing-box check` on the new config confirms `log.output` is accepted (verify empirically — established practice).
- New command `tail_engine_log(lines) -> String` + new "Логи движка" card in the Движок tab.

### 1b. App structured logs → file + live UI stream
- New `app/src-tauri/src/logging.rs`: `Logger` that (a) appends timestamped lines to `<install_dir>/tandem-app.log` with simple size-cap rotation (`.log` → `.log.1` at N MB), and (b) emits a Tauri `app.emit("log", line)` event.
- Frontend `app/src/main.js`: `listen("log", …)` appends to the existing `<pre id="log">`. Replaces today's fire-and-forget string returns with a real streamed log — the same panel, now fed live from the backend.
- Thread through `AppHandle` so long commands (`goida_test_all`, downloads, speedtest) emit progress instead of returning one final string.

### 1c. Command tracing helper
- `core/src/sys.rs`: add `run_checked(&self, cmd) -> Result<CmdOutput>` that returns `Err` with stderr on non-zero exit (today `RealSys::run` returns `Ok` regardless — the WARP silent-failure root cause).
- Every `wgcf`/`schtasks` call logs `cmd.display()` + exit code + stderr via the Logger.

**Deliverable:** every action is traceable in-app and on disk. Copy-log button already exists; wire a "copy engine log" too.

---

## Phase 2 — Fix Cloudflare WARP (small, high-value, now diagnosable)

**Root cause (confirmed from diagnostics):** log said "аккаунт создан" but `wgcf-account.toml` absent → `warp.register()?` only catches *spawn* failure, not `wgcf`'s non-zero exit. Phase 1c already converts this from "silently broken" to "here's the exact stderr."

- `core/src/warp/mod.rs`: `register`/`generate_profile` switch to `run_checked`; on failure surface wgcf's stderr. Assert the expected output file exists post-run, else `Err`.
- New `warp_diagnose` command: runs `wgcf register` with full output captured, returns it to the UI (WARP tab).
- **Once visible**, fix the actual failure. Ranked contingencies (implement in order until WARP connects):
  1. **Pin a known-good wgcf version** instead of `releases/latest` (Cloudflare periodically breaks older clients; latest isn't always compatible).
  2. Retry with backoff on transient `register` failures (429/timeouts from RU networks).
  3. Set wgcf trace/env for a clearer error; handle the "account already exists / rate-limited" case explicitly.
  4. **Stretch fallback:** native WARP registration in Rust (Cloudflare client API: generate WG keypair → POST `/reg`) removing the wgcf dependency entirely. Keeps WARP working even if wgcf rots.
- **Validation:** `warp_register` → assert both files exist → `sing-box check` the config with the WARP endpoint → `/cdn-cgi/trace` shows `warp=on` when routed.

---

## Phase 3 — Goida speedtest + geo engine (the flagship, "ULTRA ALGORITHMIC")

Replace the dumb ping filter with a real, network-aware selection engine.

### 3a. Choice: **Cloudflare Speedtest** (not Ookla)
- One HTTP surface: `__down?bytes=N` (download), `__up` (upload), `/cdn-cgi/trace` (gives `loc` = proxy **exit country**, `colo` = nearest CF datacenter, `warp` flag) — **location for free, same path**.
- Ookla has no clean public HTTP API — needs their licensed CLI, can't be cleanly pinned to a per-outbound proxy. Cloudflare wins on speed-to-build and per-server geo.

### 3b. The hard part — measuring throughput *through a specific outbound*
Clash `/proxies/{tag}/delay` measures **latency only**. To measure real Mbps per server:
- **Two-phase design** (test 5000 cheaply, throughput-test only the winners):
  - **Phase A — prefilter (all N):** existing parallel Clash `/delay` ping. Keep survivors, sort by latency, take **top-K** (configurable, default ~20).
  - **Phase B — throughput+geo (top-K only):** in the test config, give each of the K candidates its own `mixed` inbound `{tag: "probe-i", listen_port: 2100+i}` + a route rule `{"inbound":["probe-i"], "outbound": candidate.tag}`. The app then downloads `__down?bytes=25MB` and fetches `/cdn-cgi/trace` **through `http://127.0.0.1:2100+i`** (ureq proxy), timing bytes→Mbps. All K in parallel threads.
- **Validation gate:** `sing-box check` that the `inbound` route-matcher + `mixed` inbound validate (verify empirically before building on it).

### 3c. New pure core module `core/src/probe/mod.rs` (offline-testable)
- Cloudflare trace parser: `parse_trace(&str) -> Trace { loc, colo, warp, ip }`.
- Throughput math: `mbps(bytes, elapsed) -> f64`.
- **Composite scoring/ranking:** `rank(&[Sample]) -> Vec<Ranked>` where `Sample {tag, latency_ms, down_mbps, up_mbps, loc}`. Weighted score (throughput-dominant, latency-penalized, with a configurable country preference). This is the "generator tuned to *your* network" — pure, deterministic, fully unit-tested.
- Test-config assembly (per-outbound inbounds + rules) as pure functions so their JSON shape is unit-tested.

### 3d. Orchestration + UX
- New command `goida_speedtest(top_k) -> Vec<Ranked>` in the Tauri layer, emitting live progress via Phase-1 events.
- Goida tab: results table with **flag · country · latency · ↓Mbps · score**, sorted by score. Auto-select #1; manual override by click (persists via existing `active_goida_tag`). Progress bar already exists.

---

## Phase 4 — New, well-thought-out tests + CI

Current: 35 offline unit tests, **zero** integration / `sing-box check` gating. Add:

- **Unit (core, offline):** probe scoring/ranking (ties, empty, country-pref); trace parser (missing fields, `warp=off/on/plus`); Mbps math; per-outbound test-config shape; WARP profile edge cases; rules matrix (extend existing).
- **Golden config tests:** render each mode (direct / warp / goida-selected / speedtest) → assert exact JSON → **gated `#[ignore]` test runs `sing-box check` on each** when the binary is present. Turns "did I break the config?" into a test, not a live user session.
- **Error-path tests:** `run_checked` → `Err` on non-zero exit (MockSys failing responder); WARP register asserts file-existence failure.
- **Integration harness (opt-in `--ignored`):** boot sing-box with a trivial outbound, hit Clash API, assert `/delay` reachable + `mixed`-inbound proxy round-trips.
- **CI (`.github/workflows/release.yml`):** add a test job that downloads sing-box and runs the `#[ignore]`d integration + `sing-box check` gates before building installers.

---

## Phase 5 — Error-hunting methodology (cross-cutting, using codebase-memory MCP)

A repeatable ritual, not a one-off (project already indexed: 416 nodes / 1199 edges):

- **Before touching a subsystem:** `trace_path` its call graph (e.g. `install_engine → shared_route_inputs → build_rules` to prove routing invariants; every caller of `sys.run` to find unchecked-exit sites like the WARP bug).
- **Find hotspots:** `query_graph` on complexity props (`transitive_loop_depth`, `linear_scan_in_loop`) to spot the parallel speedtest's O(n) risks before they bite.
- **After each phase:** `detect_changes` for blast radius, then **re-index** so the graph stays live.
- **Bug triage template:** symptom → `search_graph` the surface → `trace_path` to the root → write a failing test → fix → re-`sing-box check`. (This is exactly how the uTLS/fingerprint/flow/routing bugs were caught — formalize it.)

---

## Suggested execution order & checkpoints
1. **Phase 1** (logging) — foundation. Checkpoint: user sees live engine+app logs.
2. **Phase 2** (WARP) — now diagnosable. Checkpoint: `warp=on` in trace.
3. **Phase 3** (speedtest engine) — flagship. Checkpoint: ranked table, blocked sites route through fastest server.
4. **Phase 4** (tests/CI) — lock it down. Checkpoint: green CI incl. `sing-box check`.
5. **Phase 5** — applied continuously from Phase 1 onward.

Each phase ends with the established gates: `cargo fmt` + `clippy -D warnings` + `cargo test` + `sing-box check` on rendered configs + rebuilt installers.

**Open decisions to confirm before build:** (a) auto-select #1 vs. always-manual pick; (b) top-K default (20?) and download size (25 MB?); (c) whether to build the native-WARP fallback (3b.4) now or defer.
