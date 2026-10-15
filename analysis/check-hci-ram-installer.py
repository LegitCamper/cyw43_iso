#!/usr/bin/env python3
"""Check stock RAM-command gates with actual ROM Thumb; never accesses hardware."""
import hashlib
from pathlib import Path
import struct

from unicorn import Uc, UC_ARCH_ARM, UC_MODE_THUMB, UC_MODE_MCLASS
from unicorn import UC_HOOK_MEM_READ, UC_HOOK_MEM_WRITE
from unicorn import arm_const as arm

ROOT = Path(__file__).resolve().parents[1]
ROM = (ROOT / "work/cyw43439-btrom-clean.bin").read_bytes()
assert hashlib.sha256(ROM).hexdigest() == "d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038"

for address, value in {
    0x3970: 0x200541, 0x3964: 0x200681,
    0x7E34: 0x209BAC, 0x7E38: 0x220000,
    0x7E3C: 0x200541, 0x7E40: 0x200682,
    0x7E44: 0xD0200, 0x7E48: 0xD8000,
    0x11E98: 0x86320,
}.items():
    assert struct.unpack_from("<I", ROM, address)[0] == value, hex(address)
for ocf, descriptor in {
    0x4C: (0x12507, 0, 6), 0x4D: (0x1250F, 8, 0),
    0x4E: (0x12525, 0, 0),
}.items():
    assert struct.unpack_from("<IHH", ROM, 0x86320 + ocf * 8) == descriptor
assert ROM[0x38DA:0x38E8].hex() == "24493d70d8f80400086022480570"


def execute(write, protected, complete, address, length, param_length=None):
    cpu = Uc(UC_ARCH_ARM, UC_MODE_THUMB | UC_MODE_MCLASS)
    cpu.mem_map(0, 0x100000)
    cpu.mem_write(0, ROM)
    cpu.mem_map(0x200000, 0x20000)
    command, result, stack, stop = 0x210000, 0x211000, 0x21D000, 0xF0000
    cpu.mem_write(0x200541, bytes([protected]))
    cpu.mem_write(0x200682, bytes([complete]))
    payload = bytes((i * 13 + 7) % 256 for i in range(length))
    parameter = struct.pack("<I", address) + (payload if write else bytes([length]))
    opcode = 0xFC4C if write else 0xFC4D
    command_bytes = struct.pack("<HB", opcode, len(parameter) if param_length is None else param_length) + parameter
    cpu.mem_write(command, command_bytes)
    cpu.mem_write(result, bytes([0xCC]) * 260)
    # All tested target bytes have mapped padding for the stock word accesses.
    begin, end = address & ~3, (address + max(length, 1) + 3) & ~3
    original = bytes((i * 3 + 0x51) % 256 for i in range(end - begin))
    cpu.mem_write(begin, original)
    accesses = []

    def memory(_cpu, access, at, size, _value, _user):
        if begin <= at < end:
            accesses.append((access, at, size))

    cpu.hook_add(UC_HOOK_MEM_READ | UC_HOOK_MEM_WRITE, memory)
    cpu.reg_write(arm.UC_ARM_REG_SP, stack)
    cpu.reg_write(arm.UC_ARM_REG_LR, stop | 1)
    cpu.reg_write(arm.UC_ARM_REG_R0, command)
    cpu.reg_write(arm.UC_ARM_REG_R1, result)
    cpu.emu_start((0x7BFA if write else 0x7D70) | 1, stop, count=100000)
    assert cpu.reg_read(arm.UC_ARM_REG_PC) == stop
    assert cpu.reg_read(arm.UC_ARM_REG_SP) == stack
    event = bytes(cpu.mem_read(result, 260))
    assert event[:1] == b"\x0e" and event[2:5] == b"\x01" + struct.pack("<H", opcode)
    return event, bytes(cpu.mem_read(begin, end - begin)), original, payload, accesses


def main():
    cases = 0
    for write in (False, True):
        for protected, complete, address, allowed in (
            (1, 1, 0x212000, False), (1, 1, 0xD1000, False),
            (1, 0, 0x212000, True), (1, 0, 0xD1000, True),
            (1, 0, 0x203A34, False), (1, 0, 0x200560, False),
            (0, 1, 0x203A34, True), (0, 1, 0x212000, True),
        ):
            event, actual, original, payload, accesses = execute(write, protected, complete, address, 4)
            assert event[5] == (0 if allowed else 0x12)
            assert event[1] == (8 if allowed and not write else 4)
            if allowed:
                assert actual == (payload if write else original)
                if not write:
                    assert event[6:10] == original
                assert accesses
            else:
                assert actual == original and not accesses
            cases += 1
        # Exercise every source/destination alignment and short tail.
        for offset in range(4):
            for length in (1, 2, 3, 4, 5, 250, 251):
                # Unicorn carries IT state past the skipped BX at 0x7d60;
                # avoid that emulator defect by ending reads on a word boundary.
                if not write and (offset + length) % 4:
                    continue
                event, actual, original, payload, accesses = execute(write, 0, 1, 0x212000 + offset, length)
                assert event[5] == 0, (write, offset, length, event[:6].hex())
                expected = original[:offset] + payload + original[offset + length:]
                assert actual == (expected if write else original)
                if not write:
                    assert event[6:6 + length] == original[offset:offset + length]
                assert all(size == 4 and at % 4 == 0 for _, at, size in accesses)
                cases += 1
    for length in (252, 253, 254, 255):
        event, actual, original, _, accesses = execute(False, 0, 1, 0x212000, length)
        assert event[5] == 0x12 and event[1] == 4
        assert actual == original and not accesses
        cases += 1
    for write, length in ((True, 3), (False, 4), (False, 6)):
        event, actual, original, _, accesses = execute(write, 0, 1, 0x212000, 4, length)
        assert event[5] == 0x12 and actual == original and not accesses
        cases += 1
    print(f"PASS: {cases} real-ROM RAM-command cases; protected post-boot access rejected, word accesses and 251-byte limit verified.")


if __name__ == "__main__":
    main()
