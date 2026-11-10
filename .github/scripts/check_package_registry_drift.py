#!/usr/bin/env python3
"""Reject package metadata that silently drops local dependency sources."""

from __future__ import annotations

import sys
import tarfile
import tomllib
from pathlib import Path


DEPENDENCY_SECTIONS = ("dependencies", "dev-dependencies", "build-dependencies")


def dependency_tables(manifest: dict) -> list[tuple[str, dict]]:
    tables: list[tuple[str, dict]] = []
    for section in DEPENDENCY_SECTIONS:
        if isinstance(manifest.get(section), dict):
            tables.append((section, manifest[section]))
    for target, values in manifest.get("target", {}).items():
        if not isinstance(values, dict):
            continue
        for section in DEPENDENCY_SECTIONS:
            if isinstance(values.get(section), dict):
                tables.append((f"target.{target}.{section}", values[section]))
    return tables


def packaged_manifest(crate_path: Path) -> dict:
    with tarfile.open(crate_path, "r:gz") as archive:
        candidates = [
            member
            for member in archive.getmembers()
            if member.isfile()
            and member.name.endswith("/Cargo.toml")
            and member.name.count("/") == 1
        ]
        if len(candidates) != 1:
            raise ValueError(
                f"expected one package manifest in {crate_path}, found {len(candidates)}"
            )
        contents = archive.extractfile(candidates[0])
        if contents is None:
            raise ValueError(f"cannot read {candidates[0].name}")
        return tomllib.load(contents)


def dependency_drift(workspace: dict, packaged: dict) -> list[str]:
    findings: list[str] = []
    for registry, patches in workspace.get("patch", {}).items():
        for name, spec in patches.items():
            findings.append(
                f"[{registry}] {name} uses a workspace-only patch ({spec}); "
                "Cargo does not include [patch] entries in the published package"
            )

    packaged_tables = dict(dependency_tables(packaged))
    for section, dependencies in dependency_tables(workspace):
        packaged_dependencies = packaged_tables.get(section, {})
        for name, spec in dependencies.items():
            if not isinstance(spec, dict) or "path" not in spec:
                continue
            normalized = packaged_dependencies.get(name)
            if not isinstance(normalized, dict) or "version" not in normalized:
                findings.append(
                    f"{section}.{name} uses local path {spec['path']}, but no "
                    "registry version was produced in the package"
                )
                continue
            findings.append(
                f"{section}.{name} uses local path {spec['path']}; consumers will "
                f"resolve registry version {normalized['version']} instead, and the "
                "local source is not verified against that release"
            )

    if packaged.get("patch"):
        findings.append("the packaged manifest unexpectedly contains [patch] entries")
    return findings


def self_test() -> None:
    workspace = {
        "patch": {"crates-io": {"gpui-pre": {"path": "vendor/gpui-pre"}}},
        "dependencies": {
            "btleplug": {
                "version": "=0.13.5",
                "path": "src/packages/ble/vendor/btleplug-0.13.5",
            }
        },
    }
    packaged = {"dependencies": {"btleplug": {"version": "=0.13.5"}}}
    findings = dependency_drift(workspace, packaged)
    assert any("workspace-only patch" in finding for finding in findings)
    assert any("registry version =0.13.5" in finding for finding in findings)
    assert not dependency_drift({}, {})
    assert dependency_drift(
        {"dependencies": {"local": {"path": "vendor/local"}}},
        {"dependencies": {"local": "0.1"}},
    ) == [
        "dependencies.local uses local path vendor/local, but no registry "
        "version was produced in the package"
    ]
    assert dependency_drift(
        {}, {"patch": {"crates-io": {"local": {"path": "vendor/local"}}}}
    ) == [
        "the packaged manifest unexpectedly contains [patch] entries"
    ]
    print("Registry drift self-test passed.")


def main() -> int:
    if sys.argv[1:] == ["--self-test"]:
        self_test()
        return 0
    if len(sys.argv) != 3:
        print(
            "usage: check_package_registry_drift.py <workspace Cargo.toml> "
            "<package directory> | --self-test",
            file=sys.stderr,
        )
        return 2

    workspace_path = Path(sys.argv[1])
    package_dir = Path(sys.argv[2])
    with workspace_path.open("rb") as manifest_file:
        workspace = tomllib.load(manifest_file)
    package = workspace["package"]
    crate_path = package_dir / f"{package['name']}-{package['version']}.crate"
    if not crate_path.is_file():
        print(f"::error title=Package missing::Expected {crate_path}")
        return 1
    packaged = packaged_manifest(crate_path)

    findings = dependency_drift(workspace, packaged)

    if not findings:
        print("Package dependency sources match the workspace release policy.")
        return 0

    print("Release blocked: package consumers would receive different dependency sources:")
    for finding in findings:
        print(f"::error title=Registry drift::{finding.replace(chr(10), ' ')}")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
