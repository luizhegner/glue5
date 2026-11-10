#!/usr/bin/env python3
"""Install full, already-converted Web AES audio archives safely.

The PC/Legacy GTA V archives are *not* compatible with the WebAssembly build:
they use the NG TOC key, even though their RPF names and byte lengths often
match the Web AES versions.  Pass only the output of `rpf7_to_aes_v2.py` here.

Files that are replaced are *moved* (not copied) to `--backup`, which must be
on the same volume, so the operation is quick and reversible: copy them back
over `data/x64/audio/sfx/` to undo it.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import sys
import time
from pathlib import Path

from rpf_audio import read_table


def is_web_aes(path: Path, key: bytes) -> bool:
    try:
        with path.open("rb") as file:
            read_table(file, path.stat().st_size, key).resolve()
        return True
    except (ValueError, RuntimeError):
        return False


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for block in iter(lambda: file.read(4 * 1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def human(size: int) -> str:
    for unit in ("B", "KB", "MB", "GB"):
        if size < 1024 or unit == "GB":
            return f"{size:.2f} {unit}" if unit != "B" else f"{size} B"
        size /= 1024
    raise AssertionError


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--pc", required=True, type=Path, help="converted Web AES game folder (contains x64/audio/sfx)")
    parser.add_argument("--data", required=True, type=Path, help="web build data folder (contains x64/audio/sfx)")
    parser.add_argument("--backup", required=True, type=Path, help="where replaced files are moved (same volume)")
    parser.add_argument("--aes-key", required=True, type=Path, help="user-supplied AES key for validating the actual name table")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)

    key = args.aes_key.read_bytes()
    if len(key) != 32:
        parser.error("--aes-key must contain 32 bytes")
    source_dir = args.pc / "x64/audio/sfx"
    target_dir = args.data / "x64/audio/sfx"
    if not source_dir.is_dir():
        print(f"error: {source_dir} not found", file=sys.stderr)
        return 2
    if not target_dir.is_dir():
        print(f"error: {target_dir} not found", file=sys.stderr)
        return 2

    files = sorted(entry for entry in source_dir.iterdir() if entry.is_file() and entry.suffix.lower() == ".rpf")
    if not files:
        print("error: source contains no RPF archives", file=sys.stderr)
        return 2
    invalid = [entry.name for entry in files if not is_web_aes(entry, key)]
    if invalid:
        print("error: source contains PC/NG or invalid audio name tables: " + ", ".join(invalid), file=sys.stderr)
        print("Use a verified converter. For old AES conversions, audit with repair_audio_name_shift.py first.", file=sys.stderr)
        return 2
    replaced = []
    added = []
    total = sum(entry.stat().st_size for entry in files)
    print(f"{len(files)} audio archives, {human(total)} from {source_dir}", flush=True)

    for index, source in enumerate(files, 1):
        target = target_dir / source.name
        backup = args.backup / source.name
        action = "add"
        if target.exists():
            if target.stat().st_size == source.stat().st_size and sha256(target) == sha256(source):
                print(f"[{index}/{len(files)}] {source.name}: identical SHA-256, skipped", flush=True)
                continue
            action = "replace"
        print(f"[{index}/{len(files)}] {source.name}: {action} ({human(source.stat().st_size)})", flush=True)
        if args.dry_run:
            continue
        backup.parent.mkdir(parents=True, exist_ok=True)
        if target.exists():
            if backup.exists():
                print(f"error: backup already exists for {source.name}; refusing to overwrite either file", file=sys.stderr)
                return 1
            os.replace(target, backup)
        partial = target.with_name(target.name + ".partial")
        shutil.copyfile(source, partial)
        os.replace(partial, target)
        shutil.copystat(source, target)
        if not is_web_aes(target, key) or sha256(target) != sha256(source):
            print(f"error: verification failed for {target}", file=sys.stderr)
            return 1
        (replaced if action == "replace" else added).append(source.name)

    manifest = {
        "source": str(source_dir),
        "target": str(target_dir),
        "backup": str(args.backup),
        "finished": time.strftime("%Y-%m-%d %H:%M:%S"),
        "replaced": replaced,
        "added": added,
    }
    if not args.dry_run:
        args.data.parent.mkdir(parents=True, exist_ok=True)
        (args.data.parent / "audio-fill.json").write_text(json.dumps(manifest, indent=1), encoding="utf-8")
    print(f"\nDone: {len(replaced)} replaced, {len(added)} added.")
    print(f"To undo: copy {args.backup}\\*.rpf back into {target_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
