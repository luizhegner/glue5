#!/usr/bin/env python3
"""Install full, already-converted Web AES audio from a zip safely.

The playgta5 build ships trimmed audio archives; so do some repacks.  This
writes each `x64/audio/sfx/*.rpf` from the zip over the data tree, one file at
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
import struct
import sys
import time
import zipfile
from pathlib import Path

PREFIX = "x64/audio/sfx/"
RPF_MAGIC = b"7FPR"  # RPF7, as little-endian bytes.
WEB_TOC_FLAG = 0x10000000
WEB_TOC_KEY = 0x0FFFFFF9


def is_web_aes(header: bytes) -> bool:
    if len(header) != 16:
        return False
    magic, _entries, names_flags, toc_key = struct.unpack("<4sIII", header)
    return magic == RPF_MAGIC and names_flags & WEB_TOC_FLAG and toc_key == WEB_TOC_KEY


def file_header(path: Path) -> bytes:
    with path.open("rb") as source:
        return source.read(16)


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
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)

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
        invalid = []
        for entry in entries:
            with archive.open(entry) as source:
                if not is_web_aes(source.read(16)):
                    invalid.append(entry.filename)
        if invalid:
            print("error: zip contains PC/NG or malformed RPFs, not Web AES:", file=sys.stderr)
            print("\n".join(invalid), file=sys.stderr)
            print("Convert the source with rpf7_to_aes_v2.py; do not extract PC/Legacy RPFs directly.", file=sys.stderr)
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
            if target.stat().st_size != entry.file_size or not is_web_aes(file_header(target)) or sha256_file(target) != source_hash:
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
