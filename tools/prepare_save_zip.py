#!/usr/bin/env python3
"""Make a small, phone-friendly zip out of a GTA V save archive.

The Goldberg archive shares a whole profile folder inside another zip; this
takes the story-mode saves (and the profile settings) and puts them at the top
level, so one "Extract" on the phone leaves a folder that can be picked by the
app's Import save button.
"""

from __future__ import annotations

import argparse
import hashlib
import sys
import tempfile
import zipfile
from pathlib import Path

README = """Save de GTA V (modo historia) para importar no Glue5

1. Envie este .zip para o celular (WhatsApp, Drive, cabo USB...).
2. No celular, abra o app Arquivos/Meus Arquivos, va em Downloads,
   toque no zip e escolha Extrair.
3. Abra o Glue5, va em Configuracoes e toque em "Import save".
4. Escolha a pasta {profile} que foi extraida.
5. Entre no modo historia e escolha Carregar jogo.

Os arquivos SGTA50000 a SGTA50015 sao os 16 slots do seu perfil.
"""

MAX_NESTED_ZIPS = 2
MAX_UNCOMPRESSED_BYTES = 1 << 30


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(4 * 1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest().upper()


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("outer", type=Path, help="downloaded archive (.zip holding the profile)")
    parser.add_argument("--profile", default="0F74F4C4", help="profile folder name (default: %(default)s)")
    parser.add_argument("--out", required=True, type=Path, help="zip to write")
    args = parser.parse_args(argv)

    def extract(archive: Path, dest: Path, depth: int = 0) -> None:
        if depth > MAX_NESTED_ZIPS:
            raise ValueError(f"archive nesting exceeds {MAX_NESTED_ZIPS} levels")
        dest.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(archive) as zipped:
            total = sum(entry.file_size for entry in zipped.infolist())
            if total > MAX_UNCOMPRESSED_BYTES:
                raise ValueError(f"archive expands to {total} bytes, above the safety limit")
            zipped.extractall(dest)
        # Shared save packs are often a zip of a zip; unpack one level at a time.
        for nested in dest.glob("*.zip"):
            extract(nested, dest / nested.stem, depth + 1)

    with tempfile.TemporaryDirectory(prefix="glue5-save-") as tmp:
        extract(args.outer, Path(tmp))

        matches = list(Path(tmp).rglob(args.profile))
        matches = [path for path in matches if path.is_dir()]
        if len(matches) != 1:
            print(f"error: expected one {args.profile} folder, found {len(matches)}", file=sys.stderr)
            return 2
        profile = matches[0]

        files = sorted(
            path for path in profile.iterdir()
            if path.is_file() and (path.name.startswith("SGTA5") or path.name == "pc_settings.bin")
        )
        if not files:
            print(f"error: no SGTA5 files in {profile}", file=sys.stderr)
            return 2

        args.out.parent.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(args.out, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
            archive.writestr("LEIA-ME.txt", README.format(profile=args.profile))
            for path in files:
                archive.write(path, f"{args.profile}/{path.name}")

    size = args.out.stat().st_size
    digest = sha256(args.out)
    print(f"{args.out}: {len(files)} files, {size / 2**20:.2f} MB")
    print(f"sha256: {digest}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
