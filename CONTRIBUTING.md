# Contributing

## Small changes, reproducible evidence

1. Describe the user-visible failure and the boundary involved.
2. Add a regression test that calls the real production function. Use `MockSys` only for the Windows boundary, never as a production fallback.
3. Run the checks below. For native changes, attach actual Windows evidence and disclose tests not run.
4. Keep the supported strategy grammar explicit; add a fixture before allowing a new flag. Never fix unsupported syntax by executing the `.bat`.

```sh
cargo fmt --all
cargo test --locked -p tandem-core --all-targets
cargo clippy --locked -p tandem-core --all-targets
cd app
npm run check
npm test
npm run build
```

Run `python scripts/verify_repository.py` from the repository root (Python 3.11+). Browser integration verification can use the optional Playwright harness described in `verification/README.md`; it is not a bundled runtime dependency.

## Review checklist

- Is the service still limited to `tandem-zapret`?
- Can user-controlled bytes become a path, command, URL, HTML, recovery target or privileged executable?
- Do failures remain failures? Are 403/404 responses distinct from transport errors?
- Does the journal precede changes, and is recovery repeatable?
- Are all file/network/process paths bounded, and are owned resources closed?
- Does the change preserve GPL attribution and avoid bundling unreviewed third-party binaries?
- Did README/API/config examples change with the implementation?

Do not add real customer data to fixtures. Do not claim passing Rust/Windows tests if only a browser fixture ran. Formatting is applied in CI so compiler parsing is exercised, but maintainers should also commit canonical `cargo fmt` output after the first native run.
