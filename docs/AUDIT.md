# Аудит исходного архива

Все номера строк ниже относятся **к оригинальному `tandem-vpn-main.zip`**, а не к переписанным файлам. Hash исходного архива: `d3769683e58cf1a3dc5c8d6dfbda892fb40934b255608534eed62fddebb45e33`.

Порядок исправления: P0 trust boundaries → P1 lifecycle/data loss → P2 functional/product debt → regression tests/docs. P0 — высокий потенциальный impact, не утверждение об уже произошедшей эксплуатации. Статус «реализовано» означает наличие исправления в source tree, а не подтверждённый Windows acceptance.

| ID | Приоритет | Оригинальные файл/строки | Дефект | Изменение | Новый модуль |
|---|---|---|---|---|---|
| A01 | P0 | core/src/zapret/mod.rs:158–165; app/src-tauri/src/lib.rs:78–80 | Service executable could be selected from an arbitrary writable directory | Fixed protected known-folder root; owner/DACL/reparse checks; no directory setter | platform.rs; workbench.rs |
| A02 | P0 | app/src-tauri/src/lib.rs:207–250 | Unpinned first ZIP asset, unbounded download and direct overwrite of active engine | Mandatory approved SHA, asset policy, bounds, staging, service-absent gate and recovery | network.rs; bundle.rs |
| A03 | P0 | app/src-tauri/src/lib.rs:255–266; core/src/hosts.rs:42–57 | Unvalidated remote hosts mappings applied with privileged write | Local reviewed import, domain/public-IP validation, backup and conflict-aware restore | hosts.rs; workbench.rs |
| A04 | P1 | core/src/zapret/mod.rs:169–175,188–231 | Nonzero command exit codes ignored; success reported after failed creation/start | Checked exit codes and native observed state; no unconditional success | sys.rs; service.rs |
| A05 | P1 | core/src/zapret/mod.rs:234–242 | Global winws process kill and shared WinDivert removal affected other products | Own-service-only removal; no taskkill or shared-driver delete | service.rs |
| A06 | P1 | core/src/zapret/mod.rs:188–210 | Delete/create/start without transition wait, rollback or ownership checks | Journaled service lifecycle and bounded state transitions | service.rs |
| A07 | P1 | core/src/zapret/mod.rs:79–107 | Localized text parsing and nonzero errors classified as not installed | SCM numeric API; only error 1060 means absence | platform.rs; service.rs |
| A08 | P1 | app/src-tauri/src/lib.rs:66–266 | Blocking process/HTTP calls executed in synchronous desktop handlers | spawn_blocking; typed action; bounded work and close-after-completion | app/src-tauri/src/lib.rs |
| A09 | P1 | app/src-tauri/src/lib.rs:17–25,78–80 | Mutex unwrap panic and mutable unvalidated installation root | No mutable directory mutex; disk config and fixed root | workbench.rs; platform.rs |
| A10 | P1 | app/src/main.js:103–209; app/src-tauri/src/lib.rs | Parallel clicks/processes could race service/files/downloads | UI gate, backend busy guard, cross-process file lock | ui-state.js; files.rs; desktop bridge |
| A11 | P1 | core/src/zapret/mod.rs:397–417 | none/any destroyed loaded IPSet; loaded did not restore original | Separate source/active lists; journaled settings; mode-preserving import | zapret/mod.rs; workbench.rs |
| A12 | P1 | core/src/hosts.rs:18–57 | Unbalanced markers could discard unmanaged content; truncate write had no backup | Reject malformed markers; atomic replacement and persistent recovery point | hosts.rs; files.rs |
| A13 | P1 | app/src-tauri/src/lib.rs:104–113 | Strategy filename accepted traversal outside installation | Single safe component, installed strategy membership, link/file validation | zapret/mod.rs; files.rs |
| A14 | P1 | core/src/zapret/strategy.rs:101–115 | First textual winws occurrence could be a comment or ambiguous invocation | Constrained invocation parser; comments ignored; exactly one command | zapret/strategy.rs |
| A15 | P1 | core/src/zapret/strategy.rs:134–152 | Unresolved variables, quoting, ports and file references unvalidated | Typed argument tokens, Windows quoting, flags/ports/files policy | zapret/strategy.rs |
| A16 | P1 | core/src/zapret/strategy.rs:55–75; tests:203–205 | Disabled game filter expanded to empty values and malformed port sets | Actual upstream disabled sentinel 12 and explicit tcp/udp/all modes | config.rs; zapret/strategy.rs |
| A17 | P1 | app/src-tauri/tauri.conf.json:24–26; capabilities/default.json:5–9 | CSP disabled and unnecessary shell capability enabled | Local CSP, main-window capability only, shell plugin removed | tauri.conf.json; capabilities/default.json |
| A18 | P1 | core/src/zapret/mod.rs:421–435; app/src-tauri/src/lib.rs:179–205 | Unbounded configurable targets and unrestricted URL/DNS/redirect behavior | 12-target HTTPS public-DNS policy; bounded workers/timeouts; no probe redirects | config.rs; network.rs |
| A19 | P1 | core/src/sys.rs:70–77 | PATH-dependent executables, unbounded waiting/output | Known System32 sc.exe; timeout, reap, dual bounded output readers | platform.rs; sys.rs |
| A20 | P1 | app/package.json; app/package-lock.json | Lock omitted declared @tauri-apps/api and CLI dependencies | Zero-dependency frontend and regenerated matching lock | app/package.json; package-lock.json |
| A21 | P2 | core/src/zapret/mod.rs:363–369; app/src/main.js:132–135 | Auto-update checkbox only wrote a flag; no startup check | Opt-in startup release check, explicitly no automatic install | Config; app/src/main.js |
| A22 | P2 | app/src/main.js:5,173–181 | Hardcoded local Zapret version was never the installed version | Read installed bundle manifest for release comparison | bundle.rs; workbench.rs |
| A23 | P2 | core/src/zapret/mod.rs:453–456 | Any unequal version was described as an available upgrade | Explicit different_release candidate; no invented semver ordering | workbench.rs; API docs |
| A24 | P2 | app/src-tauri/src/lib.rs:179–202 | HTTP status lost for 403/404; transport and HTTP failures conflated | reachable and http_ok separated, real status preserved | network.rs |
| A25 | P2 | core/src/zapret/mod.rs:139–153 | Directory errors and entry errors silently hidden as no strategies | Only NotFound becomes empty list; other errors propagate | zapret/mod.rs |
| A26 | P2 | core/src/zapret/mod.rs:259–270 | Any .sys file counted as valid driver | Specific WinDivert64.sys presence and staged PE sanity checks | zapret/mod.rs; bundle.rs |
| A27 | P2 | app/src-tauri/src/lib.rs:28–33 | Fallback to working directory and no persistent global config | Known folder and validated versioned config.json | platform.rs; config.rs |
| A28 | P2 | core/src/hosts.rs:62–135 | Tests exercised a duplicate merge implementation instead of production code | Tests call actual merge/apply/restore functions | hosts.rs tests |
| A29 | P2 | core/src/zapret/mod.rs:545–557 | Temporary test directory reused process ID, risked collisions | Process ID plus atomic sequence isolated test directories | files.rs tests |
| A30 | P2 | app/src/main.js:20–24 | Unbounded UI log growth | 200-line ring-like retained history, plain text rendering | ui-state.js |
| A31 | P2 | app/src/main.js:60–65,103–108 | Empty strategy placeholder could be treated as a selected strategy | Empty value plus selection validation and installed membership | main.js; zapret/mod.rs |
| A32 | P2 | core/src/zapret/mod.rs:168–175,209 | Global TCP timestamps changed unconditionally with no rollback | Removed implicit global network change; requirement documented | service.rs; TROUBLESHOOTING.md |
| A33 | P2 | app/src/main.js:14–15,179 | Unreliable shell-global URL opening; unnecessary capability | Release metadata displayed without shell-opening backend | main.js; IPC capability |
| A34 | P2 | .github/workflows/release.yml:11–12,32,62,69–75 | Overbroad write permission, npm install drift, wrong release tag and automatic publication | Read-only CI defaults; scoped draft-only release after checks | ci.yml; release.yml |
| A35 | P2 | README.md:20,38–50,61–62 | Universal success, unfinished features and AV bypass advice | Truthful product scope, security-first runbook, no blanket AV exclusions | README.md; docs/ |
| A36 | P2 | app/index.html:18–22; jobs.json | WARP/Goida placeholders and an empty unexplained jobs artifact | Removed non-features/artifact rather than pretending to implement new VPN engines | app/index.html; removed jobs.json |
| A37 | P2 | app/src/styles.css:144–146 | Limited narrow-screen layout and no explicit focus/reduced-motion treatment | Responsive stacked layouts, light/dark theme, focus states and 44px controls | styles.css |
| A38 | P2 | app/src-tauri/src/lib.rs; core/src/zapret/mod.rs | No redacted support export, backend audit trail or reusable CLI | Shared typed actions, CLI, bounded audit and support report | workbench.rs; core/src/bin/tandem.rs |

## Заглушки, секреты и зависимости

В исходном runtime-коде не найдено literal `TODO`, `FIXME`, `todo!` или `unimplemented!`. Это не означало функциональную полноту: auto-update был только флагом, WARP/Goida — отключёнными UI promises, а тесты hosts дублировали собственную тестовую реализацию. Эти различия отражены отдельно, без ложного «обнаружено N TODO».

В просмотренных исходниках не обнаружены встроенные API-токены, пароли или приватные ключи. Это source review и ограниченный scan, не сертифицированный secret audit истории Git: архив истории не содержит. HTTP URLs и GitHub repository identifiers не считаются секретами.

Зависимости классифицированы отдельно от подтверждённых source bugs. Удалены неиспользуемые/лишние shell и npm слои. Не заявляется, что актуальная Cargo advisory database была проверена: network/package-manager access в среде отсутствовал. Нельзя присваивать CVE только по совпадению имени библиотеки; подробности zip advisory — в [модели угроз](SECURITY.md).

## Основные новые regression cases

Неверная SHA до ZIP parsing; traversal/ADS/device names/case collisions; malformed ports/quotes/continuations; unknown command/IPC fields; mocked access-denied rollback; foreign service name collision; IPSet none-any-loaded round trip; hosts CRLF/idempotence/marker corruption/external edits; config rollback; duplicate targets; busy gate release after failure; localhost-only allowlisted frontend server.

## Не закрыто доказательствами

Native compile/Clippy/test execution, Windows default/inherited ACL behavior, service registration/reboot, real package compatibility, signed release provenance, real interrupted-operation fault injection и online dependency audit. Elevated GUI требует дальнейшего отделения privileged helper. Это реальные остающиеся gates, а не скрытые «production-ready» обещания.
