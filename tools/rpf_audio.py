"""Bounded, read-only validation of AES RPF7 name tables used by the game.

No keys or game assets are bundled. The caller supplies its own AES key.
The names-length word stores a shift in bits 28..30, not a Web-format flag:
GetEntryName resolves a name as names_base + (entry.name_offset << shift).
"""
from __future__ import annotations

import hashlib
import struct
from dataclasses import dataclass
from typing import BinaryIO

MAGIC = b"7FPR"
AES_KEY_ID = 0x0FFFFFF9
NAME_LENGTH_MASK = 0x0FFFFFFF
NAME_SHIFT_MASK = 0x70000000
MAX_TABLE = 16 * 1024 * 1024
DIRECTORY = 0x7FFFFF00


@dataclass(frozen=True)
class NameTable:
    header: bytes
    entries: bytes
    names: bytes
    size: int
    encrypted_table_sha256: str

    @property
    def shift(self) -> int:
        return (struct.unpack_from("<I", self.header, 8)[0] >> 28) & 7

    def resolve(self, shift: int | None = None) -> list[str]:
        """Validate the audio archive's flat TOC and resolve every file name.

        Require names to start on a string boundary. This conservative audio
        repair rule deliberately refuses suffix-sharing or nested archives.
        """
        shift = self.shift if shift is None else shift
        if not 0 <= shift <= 7:
            raise ValueError("name shift must be 0..7")
        names = []
        count = len(self.entries) // 16
        data_start = 16 + len(self.entries) + len(self.names)
        for index in range(count):
            entry = self.entries[index * 16:(index + 1) * 16]
            directory = struct.unpack_from("<I", entry, 4)[0] == DIRECTORY
            offset = int.from_bytes(entry[:4] if directory else entry[:2], "little") << shift
            if offset >= len(self.names):
                raise ValueError(f"entry {index}: name outside table")
            if offset and self.names[offset - 1] != 0:
                raise ValueError(f"entry {index}: name starts inside another string")
            end = self.names.find(b"\0", offset)
            if end < 0:
                raise ValueError(f"entry {index}: unterminated name")
            raw = self.names[offset:end]
            if any(byte < 32 or byte >= 127 for byte in raw):
                raise ValueError(f"entry {index}: non-ASCII audio name")
            name = raw.decode("ascii")
            if directory:
                first, children = struct.unpack_from("<II", entry, 8)
                if index != 0 or name or first != 1 or children != count - 1:
                    raise ValueError("only flat, root-directory audio archives may be repaired")
                continue
            if not name.lower().endswith(".awc") or name.lower() == ".awc" or "/" in name or "\\" in name:
                raise ValueError(f"entry {index}: not an AWC file name")
            sector = int.from_bytes(entry[5:8], "little")
            if sector & 0x800000:
                raise ValueError("resource entry in audio archive")
            stored = int.from_bytes(entry[2:5], "little") or struct.unpack_from("<I", entry, 8)[0]
            at = sector * 512
            if at < data_start or at + stored > self.size:
                raise ValueError(f"entry {index}: payload outside archive")
            names.append(name)
        if len(names) != count - 1 or len(set(name.lower() for name in names)) != len(names):
            raise ValueError("invalid or duplicate audio names")
        return names

    def corrected_header(self, shift: int) -> bytes:
        self.resolve(shift)
        header = bytearray(self.header)
        word = struct.unpack_from("<I", header, 8)[0]
        struct.pack_into("<I", header, 8, (word & ~NAME_SHIFT_MASK) | (shift << 28))
        return bytes(header)


def read_table(file: BinaryIO, size: int, key: bytes) -> NameTable:
    if len(key) != 32:
        raise ValueError("AES key must contain 32 bytes")
    file.seek(0)
    header = file.read(16)
    if len(header) != 16:
        raise ValueError("truncated RPF header")
    magic, count, word, cipher = struct.unpack("<4sIII", header)
    if magic != MAGIC or cipher != AES_KEY_ID:
        raise ValueError("expected AES RPF7; raw PC/NG archives must be converted first")
    length = count * 16 + (word & NAME_LENGTH_MASK)
    if count < 2 or not 0 < length <= MAX_TABLE or 16 + length > size:
        raise ValueError("invalid or excessive RPF table size")
    encrypted = file.read(length)
    if len(encrypted) != length:
        raise ValueError("truncated RPF table")
    try:
        from Crypto.Cipher import AES
    except ImportError as error:
        raise RuntimeError("This metadata check requires pycryptodome in the Python environment") from error
    aligned = length & ~15
    table = AES.new(key, AES.MODE_ECB).decrypt(encrypted[:aligned]) + encrypted[aligned:]
    return NameTable(header, table[:count * 16], table[count * 16:], size,
                     hashlib.sha256(encrypted).hexdigest())
