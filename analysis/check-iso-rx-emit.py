#!/usr/bin/env python3
"""Execute fixed ISO egress against stock ROM writer and .0065 Patchram."""
import argparse
import hashlib
from pathlib import Path
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def build():
    with tempfile.TemporaryDirectory(prefix="iso-rx-emit-") as directory:
        obj, elf, binary = [Path(directory) / name for name in ("emit.o", "emit.elf", "emit.bin")]
        subprocess.run(["arm-none-eabi-as", "-mcpu=cortex-m3", "-mthumb",
                        str(ROOT / "hardware/iso_rx_emit.S"), "-o", str(obj)], check=True)
        blobs = []
        for base in (0x209BAC, 0x217E78):
            subprocess.run(["arm-none-eabi-ld", f"-Ttext={base:#x}", "-e", "iso_rx_emit",
                            str(obj), "-o", str(elf)], check=True)
            subprocess.run(["arm-none-eabi-objcopy", "-O", "binary", str(elf), str(binary)], check=True)
            blobs.append(binary.read_bytes())
        assert blobs[0] == blobs[1] and len(blobs[0]) <= 160
        return blobs[0]


def check(blob):
    import unicorn as un
    from unicorn import arm_const as arm

    rom = (ROOT / "work/cyw43439-btrom-clean.bin").read_bytes()
    patch = (ROOT / "work/sdk0065_extract/segment_000d0200_000d0f90.bin").read_bytes()
    assert hashlib.sha256(rom).hexdigest() == "d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038"
    assert hashlib.sha256(patch).hexdigest() == "d9a12cb202ca648236e494b327e2d00720ade5507395a1a7fb7840db6ad586d5"
    assert patch[0xAC:0xB8].hex() == "0646e81914f108028ff701bd"
    marker = bytes(range(26))
    registers = [getattr(arm, f"UC_ARM_REG_R{i}") for i in range(13)]

    class Machine:
        def __init__(self, base, offset, available, mapped=False):
            self.cpu = cpu = un.Uc(un.UC_ARCH_ARM, un.UC_MODE_THUMB | un.UC_MODE_MCLASS)
            cpu.mem_map(0, 0x100000)
            cpu.mem_write(0, rom)
            cpu.mem_write(0xD0200, patch)
            cpu.mem_map(0x200000, 0x20000)
            cpu.mem_map(0x702000, 0x1000)
            cpu.mem_map(0x740000, 0x20000)
            cpu.mem_write(base, blob)
            self.base, self.offset = base, offset
            self.ring = 0x744000 if mapped else 0x204000
            self.output = self.ring + 4096
            self.producer, self.consumer = self.ring + 8200, self.ring + 8204
            self.put(0x2038D0, 2 if mapped else 0)
            self.put(0x2038D8, 0x204000)
            self.put(0x2038DC, 4096)
            self.put(0x2038E0, 4096)
            self.put(self.producer, offset)
            self.put(self.consumer, (offset + available) % 4096)
            cpu.mem_write(self.output, bytes([0xA5]) * 4096)
            self.put(0x702044, 0x10)
            self.events = []
            cpu.hook_add(un.UC_HOOK_CODE, self.code, begin=0x5F940, end=0x5F940)
            cpu.hook_add(un.UC_HOOK_CODE, self.code, begin=0xF0000, end=0xF0000)
            # Unicorn 2.1.4 global write hooks corrupt flags in ROM memcpy's IT tail:
            # an aligned six-byte copy writes seven bytes. Observe only these words;
            # the full-ring comparison below still checks every payload/padding byte.
            cpu.hook_add(un.UC_HOOK_MEM_WRITE, self.write,
                         begin=self.producer, end=self.producer + 3)
            cpu.hook_add(un.UC_HOOK_MEM_WRITE, self.write,
                         begin=0x702044, end=0x702047)

        def put(self, address, value):
            self.cpu.mem_write(address, struct.pack("<I", value))

        def word(self, address):
            return struct.unpack("<I", self.cpu.mem_read(address, 4))[0]

        def code(self, cpu, address, size, _):
            if address == 0xF0000:
                cpu.emu_stop()
            elif address == 0x5F940:
                # Power/clock handshake only. Writer, mapper, memcpy and notify run ROM.
                self.events.append("power")
                assert cpu.reg_read(arm.UC_ARM_REG_SP) % 8 == 0
                cpu.reg_write(arm.UC_ARM_REG_PC, cpu.reg_read(arm.UC_ARM_REG_LR))

        def write(self, cpu, access, address, size, value, _):
            if address == self.producer:
                self.events.append("publish")
            if address == 0x702044:
                self.events.append("notify")

        def emit(self, sequence=0x1234, pointer=0x210001, length=26, ipsr=0):
            cpu = self.cpu
            if 0x200000 <= pointer <= 0x21FFE6:
                cpu.mem_write(pointer, marker)
            incoming = [0x11000000 + i * 0x101 for i in range(13)]
            incoming[:3] = [sequence, pointer, length]
            for register, value in zip(registers, incoming):
                cpu.reg_write(register, value)
            cpu.reg_write(arm.UC_ARM_REG_SP, 0x21D000)
            cpu.reg_write(arm.UC_ARM_REG_LR, 0xF0001)
            cpu.reg_write(arm.UC_ARM_REG_IPSR, ipsr)
            self.events.clear()
            try:
                cpu.emu_start(self.base | 1, 0, count=10000)
            except un.UcError as error:
                raise AssertionError(f"PC={cpu.reg_read(arm.UC_ARM_REG_PC):#x}, ring={self.ring:#x}") from error
            assert cpu.reg_read(arm.UC_ARM_REG_PC) == 0xF0000, "did not return"
            assert cpu.reg_read(arm.UC_ARM_REG_SP) == 0x21D000
            assert [cpu.reg_read(reg) for reg in registers[4:12]] == incoming[4:12]
            if 0x200000 <= pointer <= 0x21FFE6:
                assert bytes(cpu.mem_read(pointer, 26)) == marker
            return cpu.reg_read(arm.UC_ARM_REG_R0)

        def expect_packet(self, sequence):
            packet = bytes([34, 0, 0, 5]) + struct.pack("<4H", 0x2042, 30, sequence, 26) + marker
            expected = bytearray([0xA5] * 4096)
            for index, value in enumerate(packet):
                expected[(self.offset + index) % 4096] = value
            actual = bytes(self.cpu.mem_read(self.output, 4096))
            assert actual == expected, (self.offset, [(i, a, b) for i, (a, b) in enumerate(zip(actual, expected)) if a != b][:12])
            assert self.word(self.producer) == (self.offset + 40) % 4096
            assert self.events == ["power", "publish", "notify"]
            assert self.word(0x702044) == 0x12

    cases = 0
    for base in (0x209BAC, 0x217E78):
        for mapped in (False, True):
            for offset in (0, 4, 4052, 4056, 4060, 4064, 4068, 4072, 4076, 4080, 4084, 4088, 4092):
                for sequence in (0, 65535):
                    machine = Machine(base, offset, 4096, mapped)
                    assert machine.emit(sequence=sequence) == 0
                    machine.expect_packet(sequence)
                    cases += 1
            # .0065 demands length+8 free bytes: 40 full, 44 accepted for this 34-byte packet.
            for available in (4, 36, 40):
                machine = Machine(base, 4080, available, mapped)
                assert machine.emit() == 1
                assert machine.word(machine.producer) == 4080
                assert bytes(machine.cpu.mem_read(machine.output, 4096)) == bytes([0xA5]) * 4096
                assert machine.events == ["power"]
                machine.put(machine.consumer, (4080 + 44) % 4096)
                assert machine.emit() == 0
                machine.expect_packet(0x1234)
                cases += 1
        for fields in ({"sequence": 65536}, {"sequence": 0xFFFFFFFF}, {"length": 0},
                       {"length": 25}, {"length": 27}, {"pointer": 0},
                       {"pointer": 0x1FFFFF}, {"pointer": 0x21FFE7},
                       {"pointer": 0xFFFFFFFF}, {"ipsr": 16}):
            machine = Machine(base, 0, 4096)
            assert machine.emit(**fields) == 2
            assert machine.events == [] and machine.word(machine.producer) == 0
            cases += 1
        for pointer in (0x200000, 0x21FFE6):
            machine = Machine(base, 0, 4096)
            assert machine.emit(pointer=pointer) == 0
            machine.expect_packet(0x1234)
            cases += 1
    print(f"ISO RX emitter: {len(blob)} PIC bytes, {cases} ROM execution checks pass; no live RX claimed")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    blob = build()
    check(blob)
    if args.output:
        args.output.write_bytes(blob)
