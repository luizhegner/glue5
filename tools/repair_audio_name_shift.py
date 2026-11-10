#!/usr/bin/env python3
"""Audit/repair the name shift in already-converted AES audio RPFs.

Default: read-only plan. --apply writes only the four-byte names-length word
after saving a rollback manifest. Audio payloads and encrypted TOCs stay intact.
Only flat audio archives with one unambiguous valid shift are accepted.
"""
from __future__ import annotations

import argparse
import json
import os
import struct
from pathlib import Path

from rpf_audio import read_table


def plan(directory: Path, key: bytes) -> list[dict]:
    files = sorted(path for path in directory.iterdir() if path.is_file() and path.suffix.lower() == ".rpf")
    if not files:
        raise ValueError("no RPF files in the specified audio directory")
    repairs = []
    for path in files:
        stat = path.stat()
        with path.open("rb") as file:
            table = read_table(file, stat.st_size, key)
        try:
            table.resolve()
            print(f"OK {path.name}: name shift {table.shift}", flush=True)
            continue
        except ValueError:
            pass
        candidates = []
        for shift in range(8):
            try:
                names = table.resolve(shift)
                candidates.append((shift, len(names)))
            except ValueError:
                continue
        if len(candidates) != 1:
            raise ValueError(f"{path.name}: invalid/ambiguous table; refusing automatic repair")
        shift, count = candidates[0]
        repairs.append({"name": path.name, "size": stat.st_size, "mtime_ns": stat.st_mtime_ns,
                        "atime_ns": stat.st_atime_ns, "table_sha256": table.encrypted_table_sha256,
                        "before": table.header.hex(), "after": table.corrected_header(shift).hex()})
        print(f"REPAIR {path.name}: name shift {table.shift} -> {shift}; {count} AWC names", flush=True)
    return repairs


def write_word(path: Path, header: bytes) -> None:
    with path.open("r+b") as file:
        file.seek(8)
        if file.write(header[8:12]) != 4:
            raise OSError("short header write")
        file.flush()
        os.fsync(file.fileno())


def verify(path: Path, item: dict, key: bytes, expected: str) -> None:
    with path.open("rb") as file:
        table = read_table(file, path.stat().st_size, key)
    if table.size != item["size"] or table.header.hex() != item[expected] or table.encrypted_table_sha256 != item["table_sha256"]:
        raise ValueError(f"{path.name}: archive changed since the plan/backup")


def apply(directory: Path, key: bytes, backup: Path, repairs: list[dict]) -> None:
    for item in repairs:
        verify(directory / item["name"], item, key, "before")
    # Exclusive creation: a previous rollback record is never overwritten.
    with backup.open("x", encoding="utf-8") as file:
        json.dump({"format": "glue5-audio-name-shift-v1", "directory": str(directory.resolve()), "files": repairs}, file, indent=2)
        file.flush()
        os.fsync(file.fileno())
    applied = []
    try:
        for item in repairs:
            path = directory / item["name"]
            # Include the current file before writing so even a partial write
            # is covered by the rollback path.
            applied.append(item)
            write_word(path, bytes.fromhex(item["after"]))
            verify(path, item, key, "after")
            with path.open("rb") as file:
                read_table(file, item["size"], key).resolve()
    except BaseException:
        for item in reversed(applied):
            path = directory / item["name"]
            write_word(path, bytes.fromhex(item["before"]))
            os.utime(path, ns=(item["atime_ns"], item["mtime_ns"]))
        raise


def restore(directory: Path, key: bytes, backup: Path) -> None:
    document = json.loads(backup.read_text(encoding="utf-8"))
    if document.get("format") != "glue5-audio-name-shift-v1" or Path(document["directory"]).resolve() != directory.resolve():
        raise ValueError("rollback record does not match this directory")
    for item in document["files"]:
        if Path(item["name"]).name != item["name"] or "/" in item["name"] or "\\" in item["name"]:
            raise ValueError("unsafe file name in rollback record")
        verify(directory / item["name"], item, key, "after")
    for item in document["files"]:
        path = directory / item["name"]
        write_word(path, bytes.fromhex(item["before"]))
        os.utime(path, ns=(item["atime_ns"], item["mtime_ns"]))
        print(f"RESTORED {item['name']}", flush=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--aes-key", required=True, type=Path)
    parser.add_argument("--backup", type=Path)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--apply", action="store_true")
    mode.add_argument("--restore", action="store_true")
    args = parser.parse_args()
    if (args.apply or args.restore) and not args.backup:
        parser.error("--apply and --restore require --backup")
    try:
        key = args.aes_key.read_bytes()
        if args.restore:
            restore(args.directory, key, args.backup)
        else:
            repairs = plan(args.directory, key)
            if args.apply and repairs:
                apply(args.directory, key, args.backup, repairs)
                print(f"Applied {len(repairs)} repairs; rollback: {args.backup}")
            else:
                print(f"Read-only plan: {len(repairs)} repairs; nothing written")
        return 0
    except (OSError, ValueError, RuntimeError) as error:
        print(f"ERROR: {error}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
