#!/usr/bin/env python3
"""Check stock code-region descriptors against retained controller evidence."""

import hashlib
from pathlib import Path
import runpy
import struct
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]
config_checks = runpy.run_path(str(ROOT / "analysis/check-config-validation.py"))


def rom_digest(data):
    # ponytail: stock lengths fit ROM's 16-bit padding length; model wrap for larger inputs.
    assert 0 < len(data) * 8 < 65536
    reversed_data = data[::-1]
    digest = (hashlib.sha256(reversed_data).digest() if len(data) % 64
              else config_checks["compression_state"](reversed_data))
    return digest[::-1]


def main():
    memory = {}
    with tempfile.TemporaryDirectory() as directory:
        subprocess.run(["cargo", "run", "--quiet", "--", "extract",
                        "work/sdk0065.bin", directory], cwd=ROOT, check=True)
        for path in Path(directory).glob("segment_*.bin"):
            address = int(path.stem.split("_")[1], 16)
            for offset, byte in enumerate(path.read_bytes(), address):
                assert offset not in memory, f"overlapping segment at {offset:#x}"
                memory[offset] = byte

    read = config_checks["read"]
    config = read(memory, 0x21f000, 0xc80)
    assert config == config_checks["extract_config"]("work/sdk0065.bin")
    assert rom_digest(config[:0xc40]).hex() == (
        "aebbd14879100d93214ebdf90be23c4bd8856df7b3dab86bb8576b0d5ba1dced")

    # ponytail: fixed .0065 descriptor table; parse signed config TLVs for other revisions.
    covered = set()
    for offset in range(0x982, 0xc32, 43):
        assert config[offset:offset + 3] == b"\x33\x01\x28"
        address, length = struct.unpack_from("<II", config, offset + 3)
        stored = config[offset + 11:offset + 43]
        data = read(memory, address, length)
        digest = rom_digest(data)
        assert stored == digest[::-1], f"descriptor mismatch at {offset:#x}"
        assert stored != digest, f"unexpected raw equality at {offset:#x}"
        region = set(range(address, address + length))
        assert not covered.intersection(region), f"overlapping descriptor at {offset:#x}"
        covered.update(region)
        print(f"config+{offset:#05x}: [{address:#010x}, {address + length:#010x}) "
              f"length={length} stored=reversed-ROM-digest")

    assert covered == (set(range(0xd0200, 0xd0f90)) |
                       set(range(0x2038d4, 0x2038d8)) |
                       set(range(0x204164, 0x204168)))
    for startup, captured in config_checks["captures"]("stock.txt"):
        assert startup == 0
        assert read(captured, 0x1921f000, 0xc80) == config
        assert read(captured, 0x192006f4, 32) == digest
        assert read(captured, 0x19200681, 2) == b"\x01\x01"
        assert read(captured, 0x19200684, 1) == b"\x01"
        assert int.from_bytes(read(captured, 0x1920062c, 4), "little") == 0x4c63
        assert int.from_bytes(read(captured, 0x1920063c, 4), "little") == 0x21f000
    print(f"PASS: 16 stock descriptors, {len(covered)} covered bytes, "
          "two live final-digest matches; no modified-image result")


if __name__ == "__main__":
    main()
