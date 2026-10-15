#!/usr/bin/env python3
"""Validate resident timer payload, ROM identity, and optional live capture."""
import argparse
import hashlib
from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--capture", type=Path)
    args = parser.parse_args()
    rom = (ROOT / "work/cyw43439-btrom-clean.bin").read_bytes()
    assert hashlib.sha256(rom).hexdigest() == "d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038"
    source = (ROOT / "hardware/picow_runtime_probe.c").read_text()
    raw = re.search(r"uint8_t controller_timer\[\] = \{(.*?)\};", source, re.S)[1]
    payload = bytes(int(v.strip(), 0) for v in raw.split(",") if v.strip())
    assert len(payload) == 256 and payload[192:] == bytes(64)
    assert payload[128:148] == b"".join(v.to_bytes(4, "little") for v in (0x742d, 0x73d5, 0x7409, 125000, 0xd035d))
    with tempfile.TemporaryDirectory() as directory:
        obj, elf, binary = (Path(directory) / name for name in ("timer.o", "timer.elf", "timer.bin"))
        subprocess.run(["arm-none-eabi-as", "-mcpu=cortex-m3", "-mthumb", str(ROOT / "hardware/controller_timer.S"), "-o", str(obj)], check=True)
        for address in (0x209bec, 0x217eb8):
            subprocess.run(["arm-none-eabi-ld", f"-Ttext={address:#x}", "-e", "hook", str(obj), "-o", str(elf)], check=True)
            subprocess.run(["arm-none-eabi-objcopy", "-O", "binary", str(elf), str(binary)], check=True)
            assert binary.read_bytes() == payload, "payload must be position independent and match embedded bytes"
        disassembly = subprocess.check_output(["arm-none-eabi-objdump", "-d", str(elf)], text=True)
        assert disassembly.count("blx\tr5") == 3
        assert "PRIMASK" in disassembly and "IPSR" in disassembly and "cpsid" in disassembly
        symbols = subprocess.check_output(["arm-none-eabi-nm", "-n", str(elf)], text=True)
        values = {name: int(address, 16) for address, _, name in (line.split() for line in symbols.splitlines())}
        assert values["trace"] - values["hook"] == 192
        assert values["timer"] - values["hook"] == 224
    if args.capture:
        text = args.capture.read_text()
        for marker in ("SRAM_PROBE result=pass", "TIMER_PROBE callback-restored=1", "TIMER_PROBE storage-retained-until-reset=1", "TIMER_PROBE result=pass"):
            assert marker in text, marker
        for phase in ("armed", "arm-repeat", "cancelled", "restored"):
            assert f"RUNTIME_PROBE TIMER_PROBE-{phase} complete=1 stock-version=1" in text
        running = re.search(r"TIMER_PROBE running status=2 first=(\d+) second=(\d+) ipsr=(\d+) arg=([0-9a-f]+) expected=([0-9a-f]+) lateness=(\d+)", text)
        assert running and 0 < int(running[1]) < int(running[2]) and int(running[3]) > 0 and running[4] == running[5]
        stopped = re.search(r"TIMER_PROBE cancelled=1 status=3 result=0 count=(\d+) later=(\d+) flags=([0-9a-f]+)", text)
        assert stopped and stopped[1] == stopped[2] and int(stopped[1]) >= int(running[2]) and int(stopped[3], 16) & 4 == 0
    print("controller timer payload, ROM identity, and requested capture checks pass")


if __name__ == "__main__":
    main()
