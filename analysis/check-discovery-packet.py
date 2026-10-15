#!/usr/bin/env python3
"""Execute discovery ARM helpers offline: uv run --with unicorn python THIS_FILE."""
from pathlib import Path
import argparse
import re
import runpy
import struct

import unicorn as un
from unicorn import arm_const as arm

ROOT = Path(__file__).resolve().parents[1]
reference = runpy.run_path(str(ROOT / "analysis/check-bis-channel.py"))["reference"]


def check(multichannel, base, native=False, continuous=False):
    name = "controller_continuous_iso" if continuous else "controller_native_iso" if native else "controller_discovery_scan" if multichannel else "controller_discovery"
    source = (ROOT / "hardware/picow_runtime_probe.c").read_text()
    raw = re.search(rf"uint8_t {name}\[\] = \{{(.*?)\}};", source, re.S)[1]
    image = bytes(int(v.strip(), 0) for v in raw.split(",") if v.strip())
    cpu = un.Uc(un.UC_ARCH_ARM, un.UC_MODE_THUMB | un.UC_MODE_MCLASS)
    cpu.mem_map(0x200000, 0x20000)
    cpu.mem_map(0x318000, 0x1000)
    cpu.mem_map(0x44000, 0x1000)
    cpu.mem_map(0x9000, 0x1000)
    cpu.mem_write(base, image)
    cpu.mem_write(0x44ed2, bytes.fromhex("7047"))  # ROM copy mocked; inputs audited.
    cpu.mem_write(0x9060, bytes.fromhex("7047"))  # Event wake mocked; inputs audited.
    copied = []
    wakes = []
    copied_tails = []

    def copy_hook(machine, address, size, data):
        if address == 0x44ed2:
            header = machine.reg_read(arm.UC_ARM_REG_R1)
            payload = machine.reg_read(arm.UC_ARM_REG_R2)
            assert machine.reg_read(arm.UC_ARM_REG_R0) == 0
            assert machine.reg_read(arm.UC_ARM_REG_R3) == 0
            assert struct.unpack("<I", machine.mem_read(machine.reg_read(arm.UC_ARM_REG_SP), 4))[0] == 0
            copied.append(header.to_bytes(2, "little") + bytes(machine.mem_read(payload, header >> 8)))
            if native:
                copied_tails.append(word(base + 1796))
                if header == 0x1a00:
                    assert payload == base + 1824 + 40 * copied_tails[-1]
                    assert bytes(machine.mem_read(payload + 26, 14)) == bytes(14)
                elif header == 1:
                    assert payload == 0
        elif address == 0x9060:
            assert machine.reg_read(arm.UC_ARM_REG_R0) == 0x2020b4
            assert machine.reg_read(arm.UC_ARM_REG_R1) == 1
            assert machine.reg_read(arm.UC_ARM_REG_R2) == 0
            wakes.append(word(base + 1816))

    cpu.hook_add(un.UC_HOOK_CODE, copy_hook)
    word = lambda address: struct.unpack("<I", cpu.mem_read(address, 4))[0]
    put = lambda address, value: cpu.mem_write(address, struct.pack("<I", value))
    if native:
        put(0x201f1c, 0x202000)

    def invoke(index):
        cpu.reg_write(arm.UC_ARM_REG_SP, 0x21ff00)
        cpu.reg_write(arm.UC_ARM_REG_LR, 0x200201)
        cpu.reg_write(arm.UC_ARM_REG_R0, base)
        cpu.reg_write(arm.UC_ARM_REG_R1, index)
        copied.clear()
        copied_tails.clear()
        cpu.emu_start((base + 1280) | 1, 0x200200, count=2000)
        assert cpu.reg_read(arm.UC_ARM_REG_PC) == 0x200200

    cases = list(range(80)) + [131073, 131074, 131075]
    time_us = 0
    for packet_index in cases:
        index = packet_index
        primary = 37
        if native:
            if index < 12:
                cycle, slot = divmod(index, 4)
                primary += slot if slot < 3 else 0
                index = 0 if slot < 3 else 1
            elif continuous:
                cycle, slot = divmod(index - 12, 5)
                if slot == 2:
                    primary += cycle % 3
                    index = 0
                elif slot == 3:
                    index = 1
                else:
                    index = 2 + 4 * cycle + {0: 0, 1: 1, 4: 3}[slot]
            else:
                index -= 10
        elif multichannel:
            if index < 3:
                primary += index
                index = 0
            else:
                index -= 2
        if index < 2:
            offset = 1024 if index == 0 else 1088
            length = image[offset + 1] + 2
            expected = bytearray(image[offset:offset + length])
            if multichannel and index == 0:
                pointer = 125 if continuous and packet_index >= 12 else (40 - primary) * 125
                expected[7:9] = pointer.to_bytes(2, "little")
            aa, crc, logical, reload = 0x8e89bed6, 0x555555, primary if index == 0 else 17, 6
            if native and index == 1:
                if continuous and packet_index >= 12:
                    expected[12:14] = (291).to_bytes(2, "little")
                    expected[28:30] = ((0x1234 + cycle + 1) & 65535).to_bytes(2, "little")
                    reload = 4
                else:
                    expected[12:14] = (2125 - cycle * 1000).to_bytes(2, "little")
                    reload = 30 if cycle < 2 else 6
        elif index % 2 == 0:
            counter = (index - 2) // 2
            expected = bytearray(image[1152:1223])
            expected[34:39] = counter.to_bytes(5, "little")
            event = counter // 2 if continuous else counter
            aa, crc, logical, reload = 0x12345678, 0xabcdef, reference((0x1234 + event) & 65535, 0x444c), 6
        else:
            counter = (index - 3) // 2
            offset = 320 + 28 * (counter % 4)
            expected = bytes([0, 26]) + image[offset:offset + 26]
            if native:
                expected = bytes([1, 0])
            aa, crc, logical, reload = 0x718ebed6, 0x123401, reference(counter & 65535, 0xcf58), 10
            if continuous and slot == 1:
                reload = 6
        invoke(packet_index)
        assert copied == [expected], (packet_index, copied, expected)
        assert (word(0x318b70), word(0x318ba4), word(0x318bf4)) == (aa, crc, logical)
        assert word(0x318b6c) & 0x300000 == 0x300000
        rf = {37: 0, 38: 24, 39: 78}.get(logical, 2 * logical + (2 if logical <= 10 else 4))
        assert word(0x3186a4) == 0x80 | rf
        assert struct.unpack("<H", cpu.mem_read(0x2097c0, 2))[0] == reload
        if native and packet_index < (80 if continuous else 14):
            if packet_index < 12:
                assert time_us == cycle * 30000 + slot * 3750
                if slot < 3:
                    assert time_us + int.from_bytes(expected[7:9], "little") * 30 == cycle * 30000 + 11250
                else:
                    assert time_us + int.from_bytes(expected[12:14], "little") * 30 == 75000
            elif continuous:
                assert time_us == 75000 + cycle * 20000 + [0, 3750, 7500, 11250, 13750][slot]
                if slot == 2:
                    assert time_us + int.from_bytes(expected[7:9], "little") * 30 == 75000 + cycle * 20000 + 11250
                elif slot == 3:
                    window = time_us + int.from_bytes(expected[12:14], "little") * 30
                    assert window <= 75000 + (cycle + 1) * 20000 < window + 30
            else:
                assert time_us == (75000 if packet_index == 12 else 78750)
            time_us += reload * 625
        if not native:
            assert bytes(cpu.mem_read(base + 1792, 256)) == bytes(256)
    if native:
        assert not wakes and word(base + 1816) == word(base + 1820) == 0
        cpu.mem_write(base + 1792, bytes(256))
        def bis_index(counter):
            return 12 + (counter // 2) * 5 + (1 if counter % 2 == 0 else 4) if continuous else 13 + counter * 2
        # Copy frees a slot, but only the next RF completion earns its credit.
        for counter in range(12):
            tail = counter % 5
            put(base + 1792, (tail + 1) % 5)
            frame = bytes([counter + 1]) * 26
            cpu.mem_write(base + 1824 + tail * 40, frame + bytes(14))
            invoke(bis_index(counter))
            assert copied == [bytes([0, 26]) + frame] and copied_tails == [tail]
            assert word(base + 1796) == (tail + 1) % 5
            assert word(base + 1808) == counter + 1
            assert word(base + 1816) == counter and word(base + 1820) == 1
            invoke(bis_index(counter) + 1)
            assert word(base + 1816) == counter + 1 and word(base + 1820) == 0
            assert wakes[-1] == counter + 1
            # Task has reported this snapshot; no duplicate wake or credit.
            put(base + 2092, counter + 1)
        wake_count = len(wakes)
        invoke(bis_index(12))
        assert copied == [bytes([1, 0])] and word(base + 1812) == 1
        invoke(bis_index(12) + 1)
        assert len(wakes) == wake_count and word(base + 1816) == 12
        # RF counter wrap and unreported completion retry do not need allocation.
        put(base + 1816, 0xffffffff)
        put(base + 2092, 0xffffffff)
        put(base + 1820, 1)
        invoke(bis_index(12) + 1)
        assert word(base + 1816) == 0 and wakes[-1] == 0
        invoke(bis_index(12) + 1)
        assert word(base + 1816) == 0 and len(wakes) == wake_count + 2
        for head, tail in ((5, 0), (0, 5), (0xffffffff, 0)):
            put(base + 1792, head)
            put(base + 1796, tail)
            invoke(bis_index(13))
            assert copied == [bytes([1, 0])]
            assert word(base + 1796) == tail and word(base + 1820) == 0
        assert bytes(cpu.mem_read(base + 2024, 24)) == bytes(24)
    print(f"Discovery ARM helper passes: multichannel={multichannel}, native={native}, continuous={continuous}, base={base:#x}, cases={len(cases)}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--native", action="store_true")
    parser.add_argument("--continuous", action="store_true")
    args = parser.parse_args()
    for mode in (False, True):
        for address in (0x209bac, 0x217e78):
            check(mode, address)
    if args.native or args.continuous:
        for address in (0x209bac, 0x217e78):
            check(True, address, native=True)
    if args.continuous:
        for address in (0x209bac, 0x217e78):
            check(True, address, native=True, continuous=True)
