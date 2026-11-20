#!/usr/bin/env python3
"""Build and package ready-to-run `wasl` and `wasld` release archives.

Produces a clean archive (`.zip` on Windows targets, `.tar.gz` elsewhere)
containing `wasl`, `wasld`, licence files, notices, and README, alongside a
`.sha256` checksum file. Never enables `miqyas` or `test-support` features.

Usage:
    python engine/scripts/package-release.py
    python engine/scripts/package-release.py --tag v0.5.0 --out-dir dist
"""

from __future__ import annotations

import argparse
import hashlib
import pathlib
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile


def detect_host_target(engine: pathlib.Path) -> str:
    out = subprocess.check_output(["rustc", "-vV"], cwd=engine, text=True)
    for line in out.splitlines():
        if line.startswith("host:"):
            return line.split(":", 1)[1].strip()
    raise SystemExit("error: could not detect rustc host target")


def workspace_version(manifest: pathlib.Path) -> str:
    data = tomllib.loads(manifest.read_text(encoding="utf-8"))
    version = data.get("workspace", {}).get("package", {}).get("version")
    if isinstance(version, str) and version:
        return version
    raise SystemExit(f"error: could not read [workspace.package].version from {manifest}")


def sha256_file(path: pathlib.Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> int:
    repo = pathlib.Path(__file__).resolve().parents[2]
    engine = repo / "engine"
    host = detect_host_target(engine)

    parser = argparse.ArgumentParser(description="Package Wasl release binaries.")
    parser.add_argument("--target", default=host, help=f"Rust target triple (default: {host})")
    parser.add_argument(
        "--tag",
        default=f"v{workspace_version(engine / 'Cargo.toml')}",
        help="Release version tag used in archive name",
    )
    parser.add_argument(
        "--out-dir",
        default=str(repo / "dist-release"),
        help="Output directory for archives and checksums",
    )
    parser.add_argument(
        "--target-dir",
        default=None,
        help="Optional Cargo target directory override",
    )
    parser.add_argument(
        "--skip-build",
        action="store_true",
        help="Package already-built release binaries without invoking cargo build",
    )
    args = parser.parse_args()

    target = args.target
    is_windows = "windows" in target
    ext = ".exe" if is_windows else ""
    out_dir = pathlib.Path(args.out_dir).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    target_dir = pathlib.Path(args.target_dir).resolve() if args.target_dir else engine / "target"

    if not args.skip_build:
        cmd = [
            "cargo",
            "build",
            "--manifest-path",
            str(engine / "Cargo.toml"),
            "--release",
            "--locked",
            "--target",
            target,
            "-p",
            "wasld",
            "--bin",
            "wasld",
            "-p",
            "wasl-cli",
            "--bin",
            "wasl",
        ]
        if args.target_dir:
            cmd.extend(["--target-dir", str(target_dir)])
        print(f"==> running (cwd={engine}): {' '.join(cmd)}")
        subprocess.run(cmd, cwd=engine, check=True)

    bin_dir = target_dir / target / "release"
    wasl_bin = bin_dir / f"wasl{ext}"
    wasld_bin = bin_dir / f"wasld{ext}"
    for b in (wasl_bin, wasld_bin):
        if not b.is_file():
            raise SystemExit(f"error: expected binary missing: {b}")

    if target == host:
        for b in (wasl_bin, wasld_bin):
            ver = subprocess.check_output([str(b), "--version"], text=True).strip()
            if "miqyas" in ver.lower():
                raise SystemExit(f"error: refusing to package test build: {ver}")
            print(f"verified: {ver}")

    stem = f"wasl-{args.tag}-{target}"
    docs = ["README.md", "LICENSE", "LICENSE-APACHE", "NOTICE.md"]

    with tempfile.TemporaryDirectory() as tmp:
        stage = pathlib.Path(tmp) / stem
        stage.mkdir()
        shutil.copy2(wasl_bin, stage / wasl_bin.name)
        shutil.copy2(wasld_bin, stage / wasld_bin.name)
        for name in docs:
            src = repo / name
            if src.is_file():
                shutil.copy2(src, stage / name)

        if is_windows:
            archive = out_dir / f"{stem}.zip"
            with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as zf:
                for p in sorted(stage.rglob("*")):
                    if p.is_file():
                        zf.write(p, p.relative_to(stage.parent))
        else:
            archive = out_dir / f"{stem}.tar.gz"
            with tarfile.open(archive, "w:gz") as tf:
                for p in sorted(stage.rglob("*")):
                    tf.add(p, arcname=p.relative_to(stage.parent).as_posix())

    digest = sha256_file(archive)
    checksum_file = out_dir / f"{archive.name}.sha256"
    checksum_file.write_text(f"{digest}  {archive.name}\n", encoding="utf-8")
    size_mib = archive.stat().st_size / (1024 * 1024)
    print(f"packaged: {archive} ({size_mib:.2f} MiB)")
    print(f"sha256:   {digest}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
