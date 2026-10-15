#!/usr/bin/env python3
"""Verify static DTM recurrence evidence; no hardware access or timing claims."""
import hashlib
from pathlib import Path
import struct


def main():
    root = Path(__file__).resolve().parents[1]
    rom = (root / "work/cyw43439-btrom-clean.bin").read_bytes()
    assert hashlib.sha256(rom).hexdigest() == "d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038"
    assert struct.unpack_from("<11I", rom, 0x8B634) == (
        0x712F3, 0x7131F, 0x71321, 0x71345, 0x71547, 0x713E3,
        0x7143F, 0x714CF, 0, 0x713FD, 0x7154F,
    )
    source, destination, size, _ = struct.unpack_from("<4I", rom, 0x89E6C)
    assert 0 <= 0x203A78 - destination < size - 11
    assert struct.unpack_from("<3I", rom, source + 0x203A78 - destination) == (20, 0, 0x8B634)
    for address, encoded in {
        0x7156E: "502101ebc00040f2712100f26930b0fbf1f07047",
        0x524A4: "20791ff062f83081",
        0x712DC: "ac4930894860b34900200860",
        0x71420: "5b494868401e48600028c8bf10bd4ff4461042694261574800894860",
        0x714CE: "10b50446c0694189491c41812d480068a5f71cfe032094f7a3fd",
        0x71500: "244b596911f0800f08bf10bdc1f30520304c0022a446401de0fb04c212096ff0240c02fb0c00c0b20a284fea400294bf921c121dd2b260f30d2159611b4942f08000086010bd",
    }.items():
        expected = bytes.fromhex(encoded)
        assert rom[address:address + len(expected)] == expected, hex(address)
    elf = (root / "work/btrom-clean-patchram.elf").read_bytes()
    # ELF32 section headers identify the patch section without relying on file offsets.
    assert elf[:7] == b"\x7fELF\x01\x01\x01"
    section_offset = struct.unpack_from("<I", elf, 32)[0]
    section_size, section_count = struct.unpack_from("<HH", elf, 46)
    patches = []
    for index in range(section_count):
        section = struct.unpack_from("<10I", elf, section_offset + index * section_size)
        if section[3] == 0xD0200:
            patches.append(elf[section[4]:section[4] + section[5]])
    assert len(patches) == 1
    assert patches[0][0x1B8:0x1DC].hex() == "38b50446c06900224179007974f787f800210091e16900230a680146184674f77cfd38bd"
    for length in range(256):
        reload = (80 + 8 * length + 873) // 625
        assert reload == (80 + 8 * length + 249 + 624) // 625
        assert reload == 1 + (length >= 38) + (length >= 116) + (length >= 194)
    assert 16 * 625 == 10000
    print("DTM recurrence static checks pass; on-air cadence and safe refill unproven.")


if __name__ == "__main__":
    main()
