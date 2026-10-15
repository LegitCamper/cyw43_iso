#!/usr/bin/env python3
"""Check clean-ROM anchors for the compact scheduler event; never touches hardware."""

import argparse
import hashlib
from pathlib import Path
import re
import struct


ROOT = Path(__file__).resolve().parents[1]
ROM_SHA256 = "d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038"


def check(path):
    rom = path.read_bytes()
    assert hashlib.sha256(rom).hexdigest() == ROM_SHA256, "wrong clean ROM"

    def word(address):
        return struct.unpack_from("<I", rom, address)[0]

    def bytes_at(address, expected):
        expected = bytes.fromhex(expected)
        assert rom[address:address + len(expected)] == expected, hex(address)

    def branch(address, target):
        first, second = struct.unpack_from("<HH", rom, address)
        assert first & 0xF800 == 0xF000, hex(address)
        assert second & 0xD000 in (0x9000, 0xD000), hex(address)
        sign = (first >> 10) & 1
        i1 = 1 ^ ((second >> 13) & 1) ^ sign
        i2 = 1 ^ ((second >> 11) & 1) ^ sign
        displacement = ((sign << 24) | (i1 << 23) | (i2 << 22)
                        | ((first & 0x3FF) << 12) | ((second & 0x7FF) << 1))
        if sign:
            displacement -= 1 << 25
        assert address + 4 + displacement == target, hex(address)

    # Reset scatter copy supplies the callback pointer, not the event storage.
    assert word(0x3120C) == 0x205DA8
    assert word(0x31210) == 0x202BBC
    assert word(0x202BBC - 0x200718 + 0x8BC8C) == 0x8AAFC
    assert struct.unpack_from("<11I", rom, 0x8AAFC) == (
        0, 0, 0x31139, 0, 0x3113F, 0, 0, 0, 0, 0x31149, 0x31207,
    )
    bytes_at(0x30F0C, "c8 21")  # Five 0x28-byte resident objects.
    branch(0x30F10, 0x821AE)
    bytes_at(0x30F20, "03 eb 85 00 01 eb c0 04 02 20 e0 74 30 20 20 74 71 20 a0 74")
    bytes_at(0x30F54, "25 f0 01 00 04 21")  # Align selected slot, add four.
    branch(0x30F5A, 0x9AEA)
    branch(0x30F62, 0xA0E8)
    bytes_at(0x30F66, "00 28 cc bf e5 61 e6 61")  # Later deadline at +0x1c.
    branch(0x30F82, 0x1C61E)

    bytes_at(0x31138, "28 21")
    branch(0x3113A, 0x8216A)  # Cleanup zeros 0x28 bytes.
    bytes_at(0x3113E, "00 29 18 bf 70 47")  # Only phase 0 does work.
    branch(0x31144, 0x1AAC0)
    bytes_at(0x31206, "00 20 70 47")  # Every accessor selector returns null.
    branch(0x3114E, 0x1CF4E)
    branch(0x3115C, 0x1CB1C)
    branch(0x311B2, 0x9C42)
    branch(0x311BE, 0x9CFE)
    branch(0x311C4, 0x9C58)
    branch(0x311CA, 0x1C27C)
    branch(0x311F8, 0x1D5B8)
    branch(0x311FE, 0x1C97E)
    bytes_at(0x1CB1C, "20 b1 00 7d 02 28 04 bf 01 20 70 47 00 20 70 47")
    assert word(0x1D0F8) == word(0xDD2C) == word(0xE06C) == 0x2042B8
    assert word(0xDD28) == word(0xE07C) == 0x202318

    # The state-14 jump-table arm runs after unlinking, then calls table +8.
    assert tuple(0xDEF2 + 2 * value for value in rom[0xDEF2:0xDEF7]) == (
        0xDF20, 0xDF00, 0xDF0C, 0xDEF8, 0xDF2C,
    )
    branch(0xDEDC, 0xC57E)
    branch(0xDEE2, 0x321C8)
    bytes_at(0xDF2C, "84 f8 14 80 20 68 41 68 00 29 1c bf 20 46 88 47 25 75")
    bytes_at(0xDF42, "20 68 81 68 00 29 1c bf 20 46 88 47")

    # Resident arena: live cursor/end, 4-byte rounding, strict remaining > size.
    assert word(0x4B1C) == 0x209BAC
    assert word(0x4B20) == 0x200560
    assert word(0x4B24) == 0x220000
    assert word(0x17AE8) == 0x200564
    assert word(0x17AF0) == 0x200568
    bytes_at(0x1785E, "10 f0 03 00 02 d0 c0 f1 04 00 04 44")
    branch(0x1786A, 0x45EE)
    bytes_at(0x17874, "01 68 30 68 09 1a a1 42 07 d9 05 46 21 46")
    branch(0x17882, 0x8216A)
    bytes_at(0x17886, "30 68 20 44 30 60")
    branch(0x17892, 0x1017E)
    branch(0x1789A, 0x9736)
    branch(0x178A0, 0x45F6)
    bytes_at(0x45EE, "ef f3 10 80 72 b6 70 47")
    bytes_at(0x45F6, "80 f3 10 88 70 47")
    # Optional pool allocation reserves scarce blocks and returns payload +4.
    bytes_at(0x179E2, "00 25 01 22 41 78 18 46")
    branch(0x179EA, 0x17910)
    branch(0x179F4, 0x178DC)
    bytes_at(0x178EC, "24 b1 6a 7c 90 42 01 d8")
    bytes_at(0x178FC, "ec 68 20 68 e8 60")
    assert word(0x17AF8) == 0x200570
    assert word(0x17B04) == 0x200574
    assert word(0x17B00) == 0x201F28
    assert rom[0x201F28 - 0x200718 + 0x8BC8C:0x201F28 - 0x200718 + 0x8BC8C + 17] == bytes.fromhex(
        "04 10 00 20 03 30 00 24 02 60 00 14 02 0c 01 08 01"
    )
    branch(0x17B24, 0x17ABE)
    bytes_at(0x17A78, "45 f8 04 0d e5 60")
    print("radio-event contract: clean-ROM callback, timing, cleanup and allocator anchors pass")


def check_capture(path):
    text = path.read_text()
    blocks = re.findall(r"^CONFIG_BEGIN startup=(\d+)\n(.*?)^CONFIG_END$", text, re.M | re.S)
    assert len(blocks) >= 2, "need two complete snapshots"
    assert text.count("CONFIG_BEGIN") == text.count("CONFIG_END") == len(blocks), "partial capture"
    expected = [(16, 32, 3), (48, 36, 2), (96, 20, 2), (268, 8, 1)]
    snapshots = []
    for status, block in blocks:
        assert status == "0", "controller startup failed"
        memory = {}
        for line in block.splitlines():
            assert "ERROR" not in line, line
            if not line.startswith("@"):
                continue
            match = re.fullmatch(r"@([0-9a-fA-F]{8}):((?:[0-9a-fA-F]{2})+)", line)
            assert match, "malformed memory record"
            address = int(match[1], 16)
            data = bytes.fromhex(match[2])
            assert 0 < len(data) <= 64, "oversized backplane record"
            assert address + len(data) <= 1 << 32, "wrapped memory record"
            for offset, value in enumerate(data):
                assert address + offset not in memory, "overlapping memory records"
                memory[address + offset] = value

        def read(address, length):
            assert all(address + i in memory for i in range(length)), f"missing {address:#x}"
            return bytes(memory[address + i] for i in range(length))

        assert struct.unpack("<I", read(0x18000C7C, 4))[0] == 0x01000100, "controller not ready"
        root = read(0x19200560, 0xA4)
        assert root[0] == 1, "pool initialization incomplete"
        cursor, end, _, pool_head = struct.unpack_from("<4I", root, 4)
        assert 0x209BAC <= cursor < end <= 0x220000 and cursor % 4 == 0, "invalid arena"
        assert pool_head == 0x200574, "unexpected first pool descriptor"
        config = read(0x19201F28, 17)
        assert config[0] == 4, "snapshot only covers four configured pools"
        assert [struct.unpack_from("<HBB", config, 1 + 4 * i) for i in range(4)] == expected
        pools = []
        for i, (size, count, reserve) in enumerate(expected):
            offset = 0x14 + 0x24 * i
            next_pool, actual_size, actual_count, base, head = struct.unpack_from("<IHHII", root, offset)
            free, actual_reserve = struct.unpack_from("<BB", root, offset + 0x10)
            minimum = struct.unpack_from("<H", root, offset + 0x1C)[0]
            assert (actual_size, actual_count, actual_reserve) == (size, count, reserve)
            assert 0 <= minimum <= free <= count and reserve < count
            assert 0x209BAC <= base < base + (size + 4) * count <= cursor and base % 4 == 0
            if free:
                assert base <= head < base + (size + 4) * count and (head - base) % (size + 4) == 0
            else:
                assert head == 0
            if i < 3:
                assert next_pool == 0x200574 + 0x24 * (i + 1)
            else:
                assert next_pool == 0 or 0x200000 <= next_pool < cursor and next_pool % 4 == 0
            pools.append((size, count, free, reserve, minimum, base, head, next_pool))
        arena = struct.unpack("<4I", read(0x192035C0, 16))
        assert 0x209BAC <= arena[0] <= arena[2] <= arena[1] <= cursor, "invalid connection arena"
        snapshots.append((cursor, end, pools, arena))
    assert all(snapshot == snapshots[0] for snapshot in snapshots), "allocator snapshots changed"
    cursor, end, pools, arena = snapshots[0]
    assert cursor <= 0x21E000 - 512 and end - cursor > 512, "512-byte candidate does not fit"
    print(f"allocator capture: {len(blocks)} matching snapshots, cursor={cursor:#x}, end={end:#x}, "
          f"gap={end - cursor}, below-config={0x21E000 - cursor}")
    print("primary pools size/count/free/reserve: " + ", ".join(
        "/".join(map(str, pool[:4])) for pool in pools))
    print(f"connection arena base={arena[0]:#x}, end={arena[1]:#x}, cursor={arena[2]:#x}; "
          "snapshot only, no allocation or SRAM execution proven")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("rom", nargs="?", type=Path, default=ROOT / "work/cyw43439-btrom-clean.bin")
    parser.add_argument("--capture", type=Path, help="validate two complete allocator snapshots")
    arguments = parser.parse_args()
    check(arguments.rom)
    if arguments.capture:
        check_capture(arguments.capture)
