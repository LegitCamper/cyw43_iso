#!/usr/bin/env python3
"""Check ROM anchors for SDIO type-5 rejection and RAM task callback; no hardware."""
import hashlib
from pathlib import Path
import struct

rom = (Path(__file__).resolve().parents[1] / "work/cyw43439-btrom-clean.bin").read_bytes()
assert hashlib.sha256(rom).hexdigest() == "d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038"
assert struct.unpack_from("<I", rom, 0x14AB8)[0] == 0x201F1C
for address, encoded in {
    0x1483C: "d5f80021002128469047",
    0x14878: "d5f80021314628469047",
    0x5FAA4: "012904d002292dd0072964d081e0",
    0x5FBB6: "4ff0010bfff722ff",
    0x5FA02: "4cf20200efe7",
    0x4418C: "0020fdf796fb",
    0x418BE: "10b50446012210210320fff78cfc8472bde81040fff7beba",
}.items():
    expected = bytes.fromhex(encoded)
    assert rom[address:address + len(expected)] == expected, hex(address)
print("SDIO ingress static checks pass; type 5 reaches Hardware Error path, RAM task callback verified.")
