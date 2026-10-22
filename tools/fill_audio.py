#!/usr/bin/env python3
"""Replace a web build's trimmed audio with the full files from a PC install.

The playgta5 build ships `data/x64/audio/sfx/*.rpf` at roughly a quarter of
their PC size (radio and score music are cut short).  Both sides are RPF7 with
the same archive version, so the PC files can be dropped in unchanged.

Files that are replaced are *moved* (not copied) to `--backup`, which must be
on the same volume, so the operation is quick and reversible: copy them back
over `data/x64/audio/sfx/` to undo it.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import time
from pathlib import Path


def human(size: int) -> str:
    for unit in ("B", "KB", "MB", "GB"):
        if size < 1024 or unit == "GB":
            return f"{size:.2f} {unit}" if unit != "B" else f"{size} B"
        size /= 1024
    raise AssertionError


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--pc", required=True, type=Path, help="PC game folder (contains x64/audio/sfx)")
    parser.add_argument("--data", required=True, type=Path, help="web build data folder (contains x64/audio/sfx)")
    parser.add_argument("--backup", required=True, type=Path, help="where replaced files are moved (same volume)")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)

    source_dir = args.pc / "x64/audio/sfx"
    target_dir = args.data / "x64/audio/sfx"
    if not source_dir.is_dir():
        print(f"error: {source_dir} not found", file=sys.stderr)
        return 2
    if not target_dir.is_dir():
        print(f"error: {target_dir} not found", file=sys.stderr)
        return 2

    files = sorted(entry for entry in source_dir.iterdir() if entry.is_file() and entry.suffix == ".rpf")
    replaced = []
    added = []
    total = sum(entry.stat().st_size for entry in files)
    print(f"{len(files)} audio archives, {human(total)} from {source_dir}", flush=True)

    for index, source in enumerate(files, 1):
        target = target_dir / source.name
        backup = args.backup / source.name
        action = "add"
        if target.exists():
            if target.stat().st_size == source.stat().st_size:
                print(f"[{index}/{len(files)}] {source.name}: already the same size, skipped", flush=True)
                continue
            action = "replace"
        print(f"[{index}/{len(files)}] {source.name}: {action} ({human(source.stat().st_size)})", flush=True)
        if args.dry_run:
            continue
        backup.parent.mkdir(parents=True, exist_ok=True)
        if target.exists() and not backup.exists():
            os.replace(target, backup)
        partial = target.with_name(target.name + ".partial")
        shutil.copyfile(source, partial)
        os.replace(partial, target)
        shutil.copystat(source, target)
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
