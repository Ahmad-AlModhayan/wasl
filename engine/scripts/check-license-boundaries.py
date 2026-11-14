#!/usr/bin/env python3
"""Verify Wasl's decoupled open-core license boundaries via `cargo metadata`.

Rules enforced:
1. Every workspace crate declares `Apache-2.0` (`wasl-proto`, `wasl-ipc`,
   `wasl-cli`) or `AGPL-3.0-only` (all core engine, daemon, storage, crypto,
   network, VFS, FFI, and relay crates).
2. No `Apache-2.0` workspace crate has a direct or transitive dependency (normal,
   build, or dev) on any `AGPL-3.0-only` workspace crate.
3. No `Apache-2.0` workspace crate depends transitively on a third-party crate
   that requires copyleft (`AGPL` / `GPL` / `LGPL` without a permissive `OR`
   alternative).
"""

from __future__ import annotations

import json
import pathlib
import re
import subprocess
import sys

APACHE_CRATES = frozenset({"wasl-proto", "wasl-ipc", "wasl-cli"})
COPYLEFT_TOKEN_RE = re.compile(r"\b(?:A|L)?GPL\b", re.IGNORECASE)


def requires_copyleft(license_expr: str | None) -> bool:
    if not license_expr:
        return True
    # Top-level SPDX `OR` / `/` alternatives mean any one branch may be chosen;
    # require at least one alternative free of AGPL/GPL/LGPL tokens.
    alternatives = re.split(r"\s+OR\s+|/", license_expr)
    return all(bool(COPYLEFT_TOKEN_RE.search(branch)) for branch in alternatives)


def load_metadata(manifest_path: pathlib.Path) -> dict:
    proc = subprocess.run(
        [
            "cargo",
            "metadata",
            "--format-version",
            "1",
            "--manifest-path",
            str(manifest_path),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        raise SystemExit(proc.returncode or 1)
    return json.loads(proc.stdout)


def check_boundaries(meta: dict) -> list[str]:
    errors: list[str] = []
    workspace_ids = set(meta.get("workspace_members", []))
    by_id = {pkg["id"]: pkg for pkg in meta.get("packages", [])}
    workspace_pkgs = {
        pkg["name"]: pkg for pkg in meta.get("packages", []) if pkg["id"] in workspace_ids
    }
    workspace_names = set(workspace_pkgs)

    for expected in sorted(APACHE_CRATES):
        if expected not in workspace_pkgs:
            errors.append(f"missing expected Apache-2.0 workspace crate: {expected}")

    for name, pkg in sorted(workspace_pkgs.items()):
        lic = pkg.get("license")
        expected = "Apache-2.0" if name in APACHE_CRATES else "AGPL-3.0-only"
        if lic != expected:
            errors.append(f"{name}: expected license={expected!r}, found {lic!r}")

    nodes = {
        node["id"]: node for node in (meta.get("resolve") or {}).get("nodes", [])
    }

    # Verify both declared Cargo.toml dependencies (normal/build/dev) and the
    # resolved Cargo graph for every Apache-2.0 workspace crate.
    for apache_name in sorted(APACHE_CRATES):
        pkg = workspace_pkgs.get(apache_name)
        if pkg is None:
            continue

        for dep in pkg.get("dependencies", []):
            dep_name = dep["name"]
            if dep_name in workspace_names and dep_name not in APACHE_CRATES:
                kind = dep.get("kind") or "normal"
                errors.append(
                    f"forbidden {kind} dependency: {apache_name} (Apache-2.0) -> "
                    f"{dep_name} ({workspace_pkgs[dep_name].get('license')})"
                )

        visited = {pkg["id"]}
        queue = [(pkg["id"], [apache_name])]
        while queue:
            cur_id, trail = queue.pop(0)
            node = nodes.get(cur_id)
            if node is None:
                continue
            for edge in node.get("deps", []):
                dep_id = edge["pkg"]
                dep_pkg = by_id.get(dep_id)
                if dep_pkg is None:
                    continue
                dep_name = dep_pkg["name"]
                if dep_id in workspace_ids and dep_name not in APACHE_CRATES:
                    errors.append(
                        f"forbidden transitive workspace dependency: "
                        f"{' -> '.join([*trail, dep_name])} "
                        f"(license={dep_pkg.get('license')!r})"
                    )
                    continue
                if dep_id not in workspace_ids and requires_copyleft(dep_pkg.get("license")):
                    errors.append(
                        f"forbidden copyleft third-party dependency in {apache_name}: "
                        f"{' -> '.join([*trail, dep_name])} "
                        f"(license={dep_pkg.get('license')!r})"
                    )
                    continue
                if dep_id not in visited:
                    visited.add(dep_id)
                    queue.append((dep_id, [*trail, dep_name]))

    return errors


def main() -> int:
    engine_dir = pathlib.Path(__file__).resolve().parent.parent
    manifest = engine_dir / "Cargo.toml"
    meta = load_metadata(manifest)
    errors = check_boundaries(meta)
    if errors:
        for err in errors:
            sys.stderr.write(f"error: {err}\n")
        return 1

    workspace_ids = set(meta.get("workspace_members", []))
    total = len(workspace_ids)
    apache_count = len(APACHE_CRATES)
    agpl_count = total - apache_count
    print(
        f"ok: verified {total} workspace crates "
        f"({apache_count} Apache-2.0, {agpl_count} AGPL-3.0-only) "
        f"with zero AGPL dependencies in Apache-2.0 crates."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
