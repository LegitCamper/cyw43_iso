#!/usr/bin/env python3
"""Verify phase-1 radio snapshot bytes and optional live capture."""
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
    raw = re.search(r"uint8_t radio_wrapper\[\] = \{(.*?)\};", source, re.S)[1]
    payload = bytes(int(value.strip(), 0) for value in raw.split(",") if value.strip())
    assert len(payload) == 48 and int.from_bytes(payload[44:48], "little") == 0x318000
    with tempfile.TemporaryDirectory() as directory:
        obj, elf, binary = (Path(directory) / name for name in ("radio.o", "radio.elf", "radio.bin"))
        subprocess.run(["arm-none-eabi-as", "-mcpu=cortex-m3", "-mthumb", str(ROOT / "hardware/event_phase_radio.S"), "-o", str(obj)], check=True)
        subprocess.run(["arm-none-eabi-ld", "-Ttext=0xd0f90", "--defsym=stock_hook=0x4917f", "-e", "hook", str(obj), "-o", str(elf)], check=True)
        subprocess.run(["arm-none-eabi-objcopy", "-O", "binary", str(elf), str(binary)], check=True)
        assert binary.read_bytes() == payload
        disassembly = subprocess.check_output(["arm-none-eabi-objdump", "-d", str(elf)], text=True)
        assert re.search(r"bl\s+4917e", disassembly)
        assert "0x714" in disassembly and "0xb68" in disassembly
        assert "adr r5, d0fc0" in disassembly
    if args.capture:
        text = args.capture.read_text()
        for marker in ("RUNTIME_PROBE execution=pass", "PHASE_RADIO global-restored=1",
                       "PHASE_RADIO scratch-retained-until-reset=1",
                       "RUNTIME_PROBE PHASE_RADIO-restored complete=1 stock-version=1",
                       "PHASE_RADIO connection status=00", "PHASE_RADIO disconnection status=00"):
            assert marker in text, marker
        assert "PHASE_RADIO shutdown-uncertain" not in text
        assert f"PHASE_RADIO stock-table={rom[0x8af40:0x8af6c].hex()}" in text
        samples = re.findall(r"PHASE_RADIO mode=([0-9a-f]{8}) headers=([0-9a-f]{8}) time=\d+", text)
        if "table-installed mode=control" in text:
            assert not samples and "PHASE_RADIO result=control-pass" in text
        else:
            assert samples and "PHASE_RADIO result=pass" in text
            assert all(int(mode, 16) != 0xffffffff for mode, _ in samples)
            print("Observed length modes:", sorted({int(mode, 16) & 3 for mode, _ in samples}))
    print("phase radio wrapper, reserved bounds and requested capture checks pass")


if __name__ == "__main__":
    main()
