#!/usr/bin/env python3
"""Install full, already-converted Web AES audio from a zip safely.

This writes each validated `x64/audio/sfx/*.rpf` from the zip over the data tree, one file at
a time (write `.partial`, then rename), so the disk never needs room for the
whole set at once.  `audio_rel.rpf`, `occlusion.rpf`, the web-only `audio.rpf`
and the custom `config/`/`tracks/` are left alone.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import sys
import time
import zipfile
from pathlib import Path

from rpf_audio import read_table

PREFIX = "x64/audio/sfx/"


def is_web_aes(source, size: int, key: bytes) -> bool:
    try:
        read_table(source, size, key).resolve()
        return True
    except (ValueError, RuntimeError):
        return False


def sha256_stream(stream) -> str:
    digest = hashlib.sha256()
    for block in iter(lambda: stream.read(4 * 1024 * 1024), b""):
        digest.update(block)
    return digest.hexdigest()


def sha256_file(path: Path) -> str:
    with path.open("rb") as source:
        return sha256_stream(source)


def human(size: int) -> str:
    value = float(size)
    for unit in ("B", "KB", "MB", "GB"):
        if value < 1024 or unit == "GB":
            return f"{value:.2f} {unit}" if unit != "B" else f"{int(value)} B"
        value /= 1024
    raise AssertionError


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--zip", required=True, type=Path, help="archive with x64/audio/sfx/*.rpf")
    parser.add_argument("--data", required=True, type=Path, help="web build data folder")
    parser.add_argument("--backup", required=True, type=Path, help="where replaced RPFs are moved (same volume as --data)")
    parser.add_argument("--aes-key", required=True, type=Path, help="user-supplied AES key for actual name-table validation")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)

    key = args.aes_key.read_bytes()
    if len(key) != 32:
        parser.error("--aes-key must contain 32 bytes")
    target_dir = args.data / "x64/audio/sfx"
    if not target_dir.is_dir():
        print(f"error: {target_dir} not found", file=sys.stderr)
        return 2

    with zipfile.ZipFile(args.zip) as archive:
        entries = [
            entry for entry in archive.infolist()
            if entry.filename.startswith(PREFIX) and entry.filename.endswith(".rpf") and not entry.is_dir()
        ]
        entries.sort(key=lambda entry: entry.filename)
        names = [entry.filename[len(PREFIX):] for entry in entries]
        if not names or any(not name or any(char in name for char in "/\\\\:") for name in names) or len({name.casefold() for name in names}) != len(names):
            print("error: zip must contain unique, flat audio archive names", file=sys.stderr)
            return 2
        invalid = []
        for entry in entries:
            with archive.open(entry) as source:
                if not is_web_aes(source, entry.file_size, key):
                    invalid.append(entry.filename)
        if invalid:
            print("error: zip contains PC/NG or invalid audio name tables:", file=sys.stderr)
            print("\n".join(invalid), file=sys.stderr)
            print("Audit converted sources with repair_audio_name_shift.py; do not extract raw PC/NG archives.", file=sys.stderr)
            return 2
        total = sum(entry.file_size for entry in entries)
        print(f"{len(entries)} archives, {human(total)} in {args.zip}", flush=True)

        for index, entry in enumerate(entries, 1):
            name = entry.filename[len(PREFIX):]
            target = target_dir / name
            backup = args.backup / name
            print(f"[{index}/{len(entries)}] {name}: {human(entry.file_size)}", flush=True)
            if args.dry_run:
                continue
            with archive.open(entry) as source:
                source_hash = sha256_stream(source)
            if target.is_file() and target.stat().st_size == entry.file_size and sha256_file(target) == source_hash:
                print("  identical SHA-256, skipped", flush=True)
                continue
            if target.exists():
                if backup.exists():
                    print(f"error: backup already exists for {name}; refusing to overwrite either file", file=sys.stderr)
                    return 1
                backup.parent.mkdir(parents=True, exist_ok=True)
                os.replace(target, backup)
            partial = target.with_name(target.name + ".partial")
            with archive.open(entry) as source, open(partial, "wb") as output:
                shutil.copyfileobj(source, output, 4 * 1024 * 1024)
            os.replace(partial, target)
            with target.open("rb") as installed:
                valid = is_web_aes(installed, target.stat().st_size, key)
            if target.stat().st_size != entry.file_size or not valid or sha256_file(target) != source_hash:
                print(f"error: verification failed for {target}", file=sys.stderr)
                return 1

    now = sum(path.stat().st_size for path in target_dir.glob("*.rpf"))
    print(f"\nNow {len(list(target_dir.glob('*.rpf')))} archives, {human(now)}.")
    if not args.dry_run:
        manifest = {
            "source": str(args.zip),
            "target": str(target_dir),
            "finished": time.strftime("%Y-%m-%d %H:%M:%S"),
            "files": [entry.filename[len(PREFIX):] for entry in entries],
        }
        (args.data.parent / "audio-fill-original.json").write_text(json.dumps(manifest, indent=1), encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
