# Changelog

## 0.2.0 — source revision, 2026-09-08

### Product

- Repositioned the misleading VPN wrapper as Tandem Workbench, a local-first operations tool for Windows IT support.
- Removed non-functional WARP/Goida tabs and unverified universal-connectivity promises.
- Added a shared CLI, typed desktop actions, redacted support report and explicit approval workflow.

### Safety and correctness

- Checked native command exit codes; bounded process and service transitions; read SCM state through Windows APIs.
- Limited service operations to owned `tandem-zapret`; removed global process kills, shared-driver deletion and implicit global TCP changes.
- Added protected root/ACL/reparse checks, bounded input, interprocess lock, operation audit and recovery journals.
- Replaced blind release extraction with pinned SHA-256, explicit asset selection, path/size policy, staging, previous-bundle retention and rollback.
- Reimplemented hosts validation/merge/restore and IPSet source/mode preservation.
- Hardened batch parsing, disabled game sentinel, filename policy, port sets, unresolved variables and file references.
- Replaced hardcoded local version with installed manifest, opt-in startup release checks, explicit tag-difference semantics.
- Separated HTTPS responses from transport failure; bounded targets/concurrency/DNS workers and restricted redirects/private DNS.

### Frontend and tooling

- Removed all external npm dependencies and built a small allowlisted localhost preview/build tool.
- Added CSP, removed shell plugin, fixed busy/error/confirmation states and bounded session logs.
- Added responsive light/dark UI, read-only standalone preview, unit/browser tests and source-level verification tooling.
- Added complete README, architecture/config/deployment/API/product/audit/recovery/security guides, CI and draft-only release workflow.

### Verification caveat

Frontend tests/build were executed locally. Native Rust compilation, Windows SCM/ACL acceptance, code signing and online dependency audit were not executed in the revision environment. See the verification report; this changelog is not a green release attestation.
