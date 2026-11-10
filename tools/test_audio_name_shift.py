"""Synthetic fixtures only: no game data or real keys are used."""
import io
import struct
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

from Crypto.Cipher import AES

import repair_audio_name_shift as repair
import fill_audio
import fill_audio_zip
from rpf_audio import read_table

KEY = bytes(range(32))


def fixture(declared_shift=1, actual_shift=0):
    names = b"\0" * (1 << actual_shift) + b"test.awc\0"
    names += b"\0" * (-len(names) % 16)
    root = struct.pack("<4I", 0, 0x7FFFFF00, 1, 1)
    entry = struct.pack("<H", 1) + b"\0\0\0" + b"\1\0\0" + struct.pack("<II", 16, 0)
    table = AES.new(KEY, AES.MODE_ECB).encrypt(root + entry + names)
    header = struct.pack("<4sIII", b"7FPR", 2, len(names) | (declared_shift << 28), 0x0FFFFFF9)
    prefix = header + table
    return prefix + b"\0" * (512 - len(prefix)) + b"ADAT" + b"\0" * 12


class AudioNameShiftTests(unittest.TestCase):
    def test_installers_validate_names_not_just_aes_magic(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "SYNTHETIC.rpf"
            for declared, actual, valid in [(0, 0, True), (1, 1, True), (1, 0, False)]:
                data = fixture(declared_shift=declared, actual_shift=actual)
                path.write_bytes(data)
                self.assertEqual(fill_audio.is_web_aes(path, KEY), valid)
                self.assertEqual(fill_audio_zip.is_web_aes(io.BytesIO(data), len(data), KEY), valid)

    def test_declared_shift_controls_name_resolution(self):
        broken = fixture()
        table = read_table(io.BytesIO(broken), len(broken), KEY)
        with self.assertRaises(ValueError):
            table.resolve()
        self.assertEqual(table.resolve(0), ["test.awc"])
        healthy = fixture(actual_shift=1)
        self.assertEqual(read_table(io.BytesIO(healthy), len(healthy), KEY).resolve(), ["test.awc"])

    def test_repair_and_restore_preserve_entire_payload(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            path = directory / "SYNTHETIC.rpf"
            original = fixture()
            path.write_bytes(original)
            stamp = path.stat().st_mtime_ns
            backup = directory / "rollback.json"
            with redirect_stdout(io.StringIO()):
                changes = repair.plan(directory, KEY)
                repair.apply(directory, KEY, backup, changes)
                fixed = path.read_bytes()
                self.assertEqual(fixed[:8], original[:8])
                self.assertEqual(fixed[12:], original[12:])
                self.assertEqual(read_table(io.BytesIO(fixed), len(fixed), KEY).resolve(), ["test.awc"])
                repair.restore(directory, KEY, backup)
            self.assertEqual(path.read_bytes(), original)
            self.assertEqual(path.stat().st_mtime_ns, stamp)

    def test_healthy_web_archive_is_not_rewritten(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "SYNTHETIC.rpf"
            original = fixture(actual_shift=1)
            path.write_bytes(original)
            with redirect_stdout(io.StringIO()):
                self.assertEqual(repair.plan(Path(tmp), KEY), [])
            self.assertEqual(path.read_bytes(), original)

    def test_failure_rolls_back_files_already_written(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            original = fixture()
            for name in ["A.rpf", "B.rpf"]:
                (directory / name).write_bytes(original)
            with redirect_stdout(io.StringIO()):
                changes = repair.plan(directory, KEY)
            real_write = repair.write_word
            calls = 0
            def fail_once(path, header):
                nonlocal calls
                calls += 1
                if calls == 2:
                    raise OSError("synthetic I/O failure")
                real_write(path, header)
            with patch.object(repair, "write_word", side_effect=fail_once):
                with self.assertRaises(OSError):
                    repair.apply(directory, KEY, directory / "rollback.json", changes)
            for name in ["A.rpf", "B.rpf"]:
                self.assertEqual((directory / name).read_bytes(), original)

    def test_raw_pc_cipher_and_wrong_key_are_refused(self):
        raw = bytearray(fixture())
        struct.pack_into("<I", raw, 12, 0x0FEFFFFF)
        with self.assertRaises(ValueError):
            read_table(io.BytesIO(raw), len(raw), KEY)
        encrypted = fixture()
        with tempfile.TemporaryDirectory() as tmp:
            (Path(tmp) / "SYNTHETIC.rpf").write_bytes(encrypted)
            with self.assertRaises(ValueError), redirect_stdout(io.StringIO()):
                repair.plan(Path(tmp), bytes(32))

    def test_existing_backup_and_changed_header_are_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            path = directory / "SYNTHETIC.rpf"
            original = fixture()
            path.write_bytes(original)
            with redirect_stdout(io.StringIO()):
                changes = repair.plan(directory, KEY)
            backup = directory / "rollback.json"
            backup.write_text("existing backup")
            with self.assertRaises(FileExistsError):
                repair.apply(directory, KEY, backup, changes)
            self.assertEqual(path.read_bytes(), original)
            self.assertEqual(backup.read_text(), "existing backup")


if __name__ == "__main__":
    unittest.main()
