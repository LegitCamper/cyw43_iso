#!/usr/bin/env python3
"""Check compact event assembly, resident bounds and optional live lifecycle."""
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
    raw = re.search(r"uint8_t controller_event\[\] = \{(.*?)\};", source, re.S)[1]
    payload = bytes(int(v.strip(), 0) for v in raw.split(",") if v.strip())
    assert len(payload) == 512 and payload[400:] == bytes(112)
    table = [int.from_bytes(payload[i:i+4], "little") for i in range(352, 396, 4)]
    assert table == [0, 0, 0, 0, 0x3113f, 0, 0, 0, 0, 0, 0x31207]
    for address, value in ((0x1a5e8, 0x201fe4), (0x1c8ac, 0x208d9c)):
        assert int.from_bytes(rom[address:address+4], "little") == value
    with tempfile.TemporaryDirectory() as directory:
        obj, elf, binary = (Path(directory) / name for name in ("event.o", "event.elf", "event.bin"))
        subprocess.run(["arm-none-eabi-as", "-mcpu=cortex-m3", "-mthumb", str(ROOT / "hardware/controller_event.S"), "-o", str(obj)], check=True)
        for address in (0x209bac, 0x217e78):
            subprocess.run(["arm-none-eabi-ld", f"-Ttext={address:#x}", "-e", "hook", str(obj), "-o", str(elf)], check=True)
            subprocess.run(["arm-none-eabi-objcopy", "-O", "binary", str(elf), str(binary)], check=True)
            assert binary.read_bytes() == payload
        symbols = subprocess.check_output(["arm-none-eabi-nm", "-n", str(elf)], text=True)
        values = {name: int(address, 16) for address, _, name in (line.split() for line in symbols.splitlines())}
        for name, offset in (("callbacks", 352), ("trace", 400), ("event", 432)):
            assert values[name] - values["hook"] == offset
        for name, value in (("clock", 0xa561), ("register_event", 0x1c61f), ("lock", 0xb4af), ("unlock", 0xb1df), ("clear", 0x31139)):
            offset = values[name] - values["hook"]
            assert int.from_bytes(payload[offset:offset+4], "little") == value
    if args.capture:
        text = args.capture.read_text()
        for marker in ("SRAM_PROBE result=pass", "EVENT_PROBE callback-restored=1", "EVENT_PROBE storage-retained-until-reset=1", "EVENT_PROBE result=pass"):
            assert marker in text, marker
        for phase in ("registered", "repeat", "restored"):
            assert f"RUNTIME_PROBE EVENT_PROBE-{phase} complete=1 stock-version=1" in text
        trace = re.search(r"EVENT_PROBE status=2 complete=1 cleanup=1 callback-ipsr=(\d+) entry-ipsr=0 primask=0 thread=([0-9a-f]+) deadline=([0-9a-f]+)", text)
        assert trace and 0x200000 <= int(trace[2], 16) < 0x220000 and int(trace[3], 16) < 1 << 27
        allocation = int(re.search(r"SRAM_PROBE allocation=([0-9a-f]+)", text)[1], 16)
        obj = re.search(r"EVENT_PROBE object=([0-9a-f]+) cleared=1 detached=1", text)
        assert obj and int(obj[1], 16) == allocation + 432
        refs = re.search(r"EVENT_PROBE refs current=([0-9a-f]+) candidate=([0-9a-f]+) incoming=([0-9a-f]+) pending=([0-9a-f]+) cursor=([0-9a-f]+) extra0=([0-9a-f]+) extra1=([0-9a-f]+) phase-owner=([0-9a-f]+)", text)
        assert refs and all(int(v, 16) != allocation + 432 for v in refs.groups())
        assert int(refs[5], 16) != allocation + 436
    print("controller event payload, resident bounds, ROM anchors and requested lifecycle checks pass")


if __name__ == "__main__":
    main()
