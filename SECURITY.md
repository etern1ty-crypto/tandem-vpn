# Security policy

This source revision is a release candidate, not a verified production release. See [the threat model](docs/SECURITY.md) and [verification boundaries](docs/VERIFICATION.md).

## Reporting

Do not disclose exploitable details, secrets, private hostnames, hosts contents or raw privileged logs in a public issue. Use private vulnerability reporting in the repository's Security tab if the maintainer has enabled it; otherwise ask the maintainers for a private reporting channel without posting the exploit. This revision does not invent a maintainer email address or promise a response SLA.

Include the version, target OS, affected trust boundary, minimal safe reproduction and whether the issue affects parsing, download integrity, filesystem permissions, recovery or service ownership. Use disposable test machines and non-sensitive fixtures. Do not test against third-party devices without authorization.

## Supported scope

Tandem does not provide VPN encryption or anonymity. Its scope is local Windows service/file operations around separately approved upstream binaries. AV alerts, driver provenance and operating-system support require independent review. Do not disable endpoint protection as a troubleshooting shortcut.
