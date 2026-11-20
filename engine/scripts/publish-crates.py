#!/usr/bin/env python3
"""Publish Wasl crates to crates.io in topological dependency order.

By default (without `--execute`), runs `cargo package --list --allow-dirty` for
every crate in order to verify manifest metadata and packaged file lists locally.
Pass `--execute` only when ready to publish to crates.io (requires
`CARGO_REGISTRY_TOKEN` or `cargo login` and setting `publish = true` on the
target crates).

Two scopes are supported:
  - `cli` (default): Apache-2.0 interface crates only (`wasl-proto`, `wasl-ipc`, `wasl-cli`).
  - `all`: full engine + daemon + CLI workspace in dependency order (excludes `wasl-ffi`).
"""

from __future__ import annotations

import argparse
import pathlib
import subprocess
import time

CLI_ORDER = [
    "wasl-proto",
    "wasl-ipc",
    "wasl-cli",
]

ALL_ORDER = [
    "wasl-proto",
    "wasl-ignore",
    "wasl-chunk",
    "wasl-crypto",
    "wasl-core",
    "wasl-fs",
    "wasl-drive",
    "wasl-store",
    "wasl-net",
    "wasl-vfs",
    "wasl-vfs-fuse",
    "wasl-vfs-windows",
    "wasl-ipc",
    "wasl-engine",
    "wasl-pairs",
    "wasl-devices",
    "wasld",
    "wasl-cli",
]


def main() -> int:
    engine = pathlib.Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description="Publish Wasl crates in dependency order.")
    parser.add_argument("--scope", choices=["cli", "all"], default="cli")
    parser.add_argument(
        "--execute",
        action="store_true",
        help="Actually publish to crates.io instead of verifying packages locally",
    )
    parser.add_argument("--sleep", type=int, default=20, help="Seconds to wait between crates on crates.io")
    args = parser.parse_args()

    order = CLI_ORDER if args.scope == "cli" else ALL_ORDER
    for idx, crate in enumerate(order):
        if args.execute:
            cmd = [
                "cargo",
                "publish",
                "--manifest-path",
                str(engine / "Cargo.toml"),
                "-p",
                crate,
                "--locked",
            ]
        else:
            cmd = [
                "cargo",
                "package",
                "--manifest-path",
                str(engine / "Cargo.toml"),
                "-p",
                crate,
                "--list",
                "--allow-dirty",
            ]
        print(f"==> [{idx + 1}/{len(order)}] {crate}: {' '.join(cmd)}")
        subprocess.run(cmd, cwd=engine, check=True)
        if args.execute and idx + 1 < len(order):
            time.sleep(args.sleep)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
