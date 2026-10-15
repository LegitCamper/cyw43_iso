#!/usr/bin/env python3
"""Check static CYW43439 whitening-candidate evidence; never accesses hardware."""
import hashlib
from pathlib import Path
import struct


def main():
    rom = (Path(__file__).resolve().parents[1] / "work/cyw43439-btrom-clean.bin").read_bytes()
    assert hashlib.sha256(rom).hexdigest() == "d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038"
    source, destination, size, copy = struct.unpack_from("<4I", rom, 0x89E6C)
    assert (source, destination, size, copy) == (0x8BC8C, 0x200718, 0x3590, 0x488)
    assert 0 <= 0x203154 - destination < size
    assert rom[source + 0x203154 - destination] == 0x19
    for address, value in {
        0x447A4: 0x203154, 0x447A8: 0x318B6C, 0x447AC: 0x318BF4,
        0x4767C: 0x01002140, 0x47680: 0x318B6C, 0x47688: 0x207130,
        0x71598: 0x01002340, 0x7159C: 0x318B6C,
    }.items():
        assert struct.unpack_from("<I", rom, address)[0] == value, hex(address)
    for address, encoded in {
        # Global bit 0 selects the 0x300000 mask and channel-register write.
        0x44450: "d449097811f0010fd34904bf086820f4401004d00a6842f440120a60cf4908607047",
        # Channel byte followed by BLE channel-to-frequency mapping.
        0x47238: "11f0200f18bf70bd84f883000a284fea400094bf801c001d84f87a00",
        # State bit 3 controls bit 0x200 in normal radio setup.
        0x4759E: "3a48807c10f0080f03d0286840f400702860",
        # Allocate when null; set/clear state bit 3 from allocation outcome.
        0x475C6: "10b52f4ca06840b92af048fea060002804bfa07c40f0080002d0a07c20f00800a07410bd",
        # Dynamic normal-connection paths set and clear bit 0x200.
        0x44834: "f648016841f400710160",
        0x494FE: "e148016821f400710160",
    }.items():
        expected = bytes.fromhex(encoded)
        assert rom[address:address + len(expected)] == expected, hex(address)
    print("Whitening-candidate static checks pass; hardware semantics unproven.")


if __name__ == "__main__":
    main()
