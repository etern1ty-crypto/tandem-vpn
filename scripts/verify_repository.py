#!/usr/bin/env python3
"""Offline structural checks; deliberately not a Rust compiler or CVE scan."""
from pathlib import Path
import argparse
import hashlib
import json
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]

def verify(release_tag=None):
    errors = []
    checks = []
    def check(condition, message):
        if condition: checks.append(message)
        else: errors.append(message)
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
    package = json.loads((ROOT / "app/package.json").read_text(encoding="utf-8"))
    node_lock = json.loads((ROOT / "app/package-lock.json").read_text(encoding="utf-8"))
    tauri = json.loads((ROOT / "app/src-tauri/tauri.conf.json").read_text(encoding="utf-8"))
    version = workspace["workspace"]["package"]["version"]
    check(version == package["version"] == tauri["version"] == node_lock["version"], "Versions are consistent")
    if release_tag: check(release_tag == f"v{version}", "Release tag matches version")
    check(not package.get("dependencies") and not package.get("devDependencies"), "No external npm dependencies")
    check(list(node_lock["packages"]) == [""], "Node lock contains only the local package")
    check(node_lock["packages"][""]["name"] == package["name"], "Node manifest and lock names agree")
    check(tauri["app"]["security"]["csp"] is not None and "object-src 'none'" in tauri["app"]["security"]["csp"], "Desktop CSP is explicit")
    capability = json.loads((ROOT / "app/src-tauri/capabilities/default.json").read_text(encoding="utf-8"))
    check(not any("shell" in p for p in capability["permissions"]), "No shell capability")
    check(capability["windows"] == ["main"], "Capabilities are scoped to main window")
    packages = {(p["name"],p["version"]):p for p in lock["package"]}
    names = {}
    for key in packages: names.setdefault(key[0], []).append(key)
    for key, value in packages.items():
        if "source" in value: check(bool(re.fullmatch(r"[a-f0-9]{64}", value.get("checksum", ""))), f"Registry checksum shape: {key[0]} {key[1]}")
        for dependency in value.get("dependencies", []):
            words = dependency.split(); candidates = names.get(words[0], [])
            if len(words) > 1: candidates = [key for key in candidates if key[1] == words[1]]
            check(len(candidates) == 1, f"Lock dependency resolves: {dependency}")
    for name in ["tandem-core", "tandem-app"]: check((name,version) in packages, f"Local Cargo package version: {name}")
    check("tauri-plugin-shell" not in names and "zip-extract" not in names, "Removed shell and blind extraction dependencies")
    for file in [ROOT / "core/Cargo.toml", ROOT / "app/src-tauri/Cargo.toml"]:
        crate = tomllib.loads(file.read_text(encoding="utf-8"))
        local = packages[(crate["package"]["name"],version)]
        declared = set(crate.get("dependencies", {})) | set(crate.get("build-dependencies", {})) | set(crate.get("dev-dependencies", {}))
        locked = {d.split()[0] for d in local.get("dependencies", [])}
        check(declared == locked, f"Direct dependency names agree: {file.relative_to(ROOT)}")
        for name, spec in crate.get("dependencies", {}).items():
            wanted = spec if isinstance(spec, str) else spec.get("version")
            if wanted and wanted.startswith("="):
                check((name,wanted[1:]) in packages, f"Pinned direct version is available in lock: {name}")
    for path in ROOT.rglob("*.md"):
        if any(part in {"node_modules","target","dist"} for part in path.parts): continue
        text = path.read_text(encoding="utf-8")
        for url in re.findall(r"\]\(([^)\s]+)\)",text):
            if re.match(r"^(?:https?://|mailto:|#|\{\{)",url): continue
            destination = (path.parent / url.split("#")[0]).resolve()
            check(destination.exists(), f"Documentation link resolves: {path.relative_to(ROOT)} -> {url}")
    html = (ROOT / "app/index.html").read_text(encoding="utf-8")
    ids = re.findall(r'\bid="([^"]+)"',html)
    check(len(ids) == len(set(ids)), "HTML IDs are unique")
    js = (ROOT / "app/src/main.js").read_text(encoding="utf-8")
    used = set(re.findall(r'\$\("([^"]+)"\)',js)) | set(re.findall(r'bind\("([^"]+)"',js))
    check(used <= set(ids), "Every frontend element reference exists")
    for path in list((ROOT / "core/src").rglob("*.rs")) + list((ROOT / "app/src-tauri/src").rglob("*.rs")):
        text = path.read_text(encoding="utf-8")
        check(not re.search(r"\b(?:todo!|unimplemented!)\s*\(",text), f"No runtime TODO/unimplemented macro: {path.relative_to(ROOT)}")
    check(not (ROOT / "jobs.json").exists(), "Empty jobs artifact removed")
    if errors:
        print(json.dumps({"status":"failed","errors":errors},ensure_ascii=False,indent=2)); return 1
    print(json.dumps({"status":"passed","structural_check_count":len(checks),"rust_compilation_verified":False,"advisory_scan_verified":False,"frontend_external_packages":len(node_lock["packages"])-1,"cargo_package_count":len(packages)},ensure_ascii=False,indent=2)); return 0

if __name__ == "__main__":
    parser=argparse.ArgumentParser(); parser.add_argument("--release-tag")
    sys.exit(verify(parser.parse_args().release_tag))
