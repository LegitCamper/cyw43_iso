#!/usr/bin/env python3
"""Check native CSA #2 and compile a relocation-free Cortex-M3 routine.

No hardware execution: native exhaustive checks plus ARM compiler inspection.
Use --output DIR to retain bis_channel.{o,elf,bin,txt} for the radio owner.
"""
import argparse
import ctypes
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def reference(counter, channel_id):
    value = counter ^ channel_id
    for _ in range(3):
        value = (int(f"{value & 255:08b}"[::-1], 2)
                 | (int(f"{value >> 8:08b}"[::-1], 2) << 8))
        value = (17 * value + channel_id) & 65535
    return (value ^ channel_id) % 37


def check(directory):
    source = ROOT / "hardware/bis_channel.c"
    library = directory / "bis_channel.so"
    subprocess.run(["cc", "-std=c11", "-O2", "-shared", "-fPIC", "-Wall", "-Wextra",
                    "-Werror", str(source), "-o", str(library)], check=True)
    native = ctypes.CDLL(str(library)).bis_channel
    native.argtypes = [ctypes.c_uint32, ctypes.c_uint32]
    native.restype = ctypes.c_uint32
    assert [native(i, 0x305f) for i in range(4)] == [25, 20, 6, 21]
    for channel_id in (0, 0x305f, 0xcf58, 0xffff):
        for counter in range(65536):
            assert native(counter, channel_id) == reference(counter, channel_id)
        assert native(65536, channel_id) == native(0, channel_id)
    obj, elf, binary = (directory / f"bis_channel.{ext}" for ext in ("o", "elf", "bin"))
    subprocess.run(["arm-none-eabi-gcc", "-std=c11", "-O2", "-mcpu=cortex-m3", "-mthumb",
                    "-ffreestanding", "-fPIC", "-fno-builtin", "-Wall", "-Wextra", "-Werror",
                    "-c", str(source), "-o", str(obj)], check=True)
    assert not subprocess.check_output(["arm-none-eabi-nm", "-u", str(obj)], text=True).strip()
    previous = None
    for address in (0x209bac, 0x217e78):
        subprocess.run(["arm-none-eabi-ld", f"-Ttext={address:#x}", "-e", "bis_channel",
                        str(obj), "-o", str(elf)], check=True)
        subprocess.run(["arm-none-eabi-objcopy", "-O", "binary", "--only-section=.text",
                        str(elf), str(binary)], check=True)
        current = binary.read_bytes()
        if previous is not None:
            assert previous == current, "code depends on load address"
        previous = current
    disassembly = subprocess.check_output(["arm-none-eabi-objdump", "-d", str(elf)], text=True)
    (directory / "bis_channel.txt").write_text(disassembly)
    print(f"262144 native/reference cases pass; ARM routine {len(previous)} bytes, no undefined symbols")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.output:
        args.output.mkdir(parents=True, exist_ok=True)
        check(args.output.resolve())
    else:
        with tempfile.TemporaryDirectory() as temporary:
            check(Path(temporary))
