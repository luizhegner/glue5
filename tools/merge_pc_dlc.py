#!/usr/bin/env python3
"""Add DLC packs from a PC GTA V install to a web-build data tree.

The web build does not use a special asset format: ``data/x64/dlcPacks/<name>/dlc.rpf``
is exactly the ``update/x64/dlcpacks/<name>/dlc.rpf`` file the PC install ships.
Only two things are needed:

* copy the ``dlc.rpf`` files the target build does not have, and
* list them in ``data/common/data/dlclist.xml``.

Nothing is re-encoded and no archive is rebuilt, so this is a plain file copy.
Run with ``--dry-run`` first: it prints the plan without touching anything.

The PC install's ``update.rpf`` also holds newer versions of core files
(``common/data/*.meta``, levels, ...).  This script deliberately does not merge
those; mixing a newer update.rpf into older base data is a much larger change
and is not needed to add a DLC pack.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import sys
import time
from pathlib import Path

DLCLIST = Path("common/data/dlclist.xml")
DLC_DIR = Path("x64/dlcPacks")
ITEM = re.compile(r"<Item>\s*platform:\\(?:/)?dlcPacks\\([^<\\/]+)\\?\s*</Item>", re.I)
RPF7 = b"7FPR"


def human(size: int) -> str:
    for unit in ("B", "KB", "MB", "GB"):
        if size < 1024 or unit == "GB":
            return f"{size:.2f} {unit}" if unit != "B" else f"{size} B"
        size /= 1024
    raise AssertionError


def read_list(path: Path) -> tuple[str, list[str]]:
    text = path.read_text(encoding="utf-8-sig")
    return text, [m.group(1) for m in ITEM.finditer(text)]


def candidate_packs(pc: Path) -> list[tuple[str, Path, int]]:
    root = pc / "update/x64/dlcpacks"
    found = []
    if not root.is_dir():
        return found
    for entry in sorted(root.iterdir()):
        if not entry.is_dir():
            continue
        rpf = entry / "dlc.rpf"
        if rpf.is_file():
            found.append((entry.name, rpf, rpf.stat().st_size))
    return found


def check_rpf(path: Path) -> str | None:
    with path.open("rb") as handle:
        magic = handle.read(4)
    if magic != RPF7:
        return f"not an RPF7 archive (starts with {magic!r})"
    return None


def write_list(path: Path, text: str, found: list[str]) -> None:
    stamp = time.strftime("%Y%m%d-%H%M%S")
    backup = path.with_name(f"{path.name}.bak-{stamp}")
    shutil.copy2(path, backup)

    indent = "\t\t"
    lines = []
    for name in found:
        lines.append(f"{indent}<Item>platform:\\dlcPacks\\{name}\\</Item>")
    addition = "\n".join(lines) + "\n"

    if "</Paths>" not in text:
        raise SystemExit(f"{path} has no </Paths> element; is this a dlclist.xml?")
    updated = text.replace("</Paths>", addition + "\t</Paths>", 1)
    path.write_text(updated, encoding="utf-8")
    print(f"  dlclist.xml updated ({len(found)} entries); backup: {backup.name}")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--pc", required=True, type=Path, help="PC game folder (contains update/ and x64a.rpf)")
    parser.add_argument("--data", required=True, type=Path, help="web build folder (contains common/ and x64/)")
    parser.add_argument("--only", nargs="*", metavar="NAME", help="merge only these packs")
    parser.add_argument("--force", action="store_true", help="replace packs that are already present")
    parser.add_argument("--dry-run", action="store_true", help="print the plan and change nothing")
    args = parser.parse_args(argv)

    dlclist = args.data / DLCLIST
    if not dlclist.is_file():
        print(f"error: {dlclist} not found; --data must be the build's data folder", file=sys.stderr)
        return 2
    if not (args.pc / "update/x64/dlcpacks").is_dir():
        print(f"error: {args.pc / 'update/x64/dlcpacks'} not found; --pc must be the PC game folder", file=sys.stderr)
        return 2

    text, listed = read_list(dlclist)
    listed_lower = {name.lower() for name in listed}
    present = {name.lower(): name for name in listed}
    target_root = args.data / DLC_DIR

    packs = candidate_packs(args.pc)
    if args.only:
        wanted = {name.lower() for name in args.only}
        packs = [pack for pack in packs if pack[0].lower() in wanted]
        unknown = wanted - {pack[0].lower() for pack in packs}
        if unknown:
            print(f"error: not found on the PC install: {', '.join(sorted(unknown))}", file=sys.stderr)
            return 2

    to_copy = []
    to_register = []
    skipped = []
    for name, source, size in packs:
        target = target_root / name / "dlc.rpf"
        exists = target.is_file()
        if exists and not args.force:
            skipped.append((name, "already present", size))
            if name.lower() not in listed_lower:
                to_register.append(name)
            continue
        reason = check_rpf(source)
        if reason:
            skipped.append((name, reason, size))
            continue
        to_copy.append((name, source, target, size, exists))
        if name.lower() not in listed_lower:
            to_register.append(name)

    print(f"PC install:  {args.pc}")
    print(f"web data:    {args.data}")
    print(f"DLC packs on the PC install: {len(packs)}")
    print()
    if to_copy:
        print(f"Will copy {len(to_copy)} pack(s), {human(sum(item[3] for item in to_copy))}:")
        for name, _source, target, size, replaces in to_copy:
            note = " (replaces existing)" if replaces else ""
            print(f"  {name:<20} {human(size):>10}{note} -> {target.relative_to(args.data)}")
    else:
        print("Nothing to copy.")
    if to_register:
        print(f"\nWill add {len(to_register)} entr{'y' if len(to_register) == 1 else 'ies'} to dlclist.xml: {', '.join(to_register)}")
    if skipped:
        print("\nSkipped:")
        for name, reason, size in skipped:
            print(f"  {name:<20} {human(size):>10}  {reason}")

    if args.dry_run:
        print("\nDry run: nothing was changed.")
        return 0

    for name, source, target, size, _replaces in to_copy:
        target.parent.mkdir(parents=True, exist_ok=True)
        partial = target.with_name("dlc.rpf.partial")
        print(f"copying {name} ({human(size)}) ...", flush=True)
        shutil.copyfile(source, partial)
        os.replace(partial, target)
        shutil.copystat(source, target)

    if to_register:
        write_list(dlclist, text, to_register)

    print("\nDone. Restart the glue5 server so it rescans data/.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
