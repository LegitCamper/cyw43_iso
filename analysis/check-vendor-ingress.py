#!/usr/bin/env python3
"""Check vendor SDU producer bounds/ABI; --emulate executes Thumb with Unicorn."""
import argparse
from collections import deque
import hashlib
from pathlib import Path
import random
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
ROM_SHA = "d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038"


def build(directory, address):
    obj, elf, binary = (directory / name for name in ("ingress.o", "ingress.elf", "ingress.bin"))
    subprocess.run(["arm-none-eabi-as", "-mcpu=cortex-m3", "-mthumb",
                    str(ROOT / "hardware/iso_vendor_ingress.S"), "-o", str(obj)], check=True)
    subprocess.run(["arm-none-eabi-ld", f"-Ttext={address:#x}", "-e", "vendor_lookup",
                    str(obj), "-o", str(elf)], check=True)
    subprocess.run(["arm-none-eabi-objcopy", "-O", "binary", str(elf), str(binary)], check=True)
    names = subprocess.check_output(["arm-none-eabi-nm", "-n", str(elf)], text=True)
    symbols = {name: int(value, 16) for value, _, name in (line.split() for line in names.splitlines())}
    disassembly = subprocess.check_output(["arm-none-eabi-objdump", "-d", str(elf)], text=True)
    return binary.read_bytes(), symbols, disassembly


def static_checks(blob, symbols, disassembly, base):
    assert len(blob) == 416
    for name, offset in (("vendor_lookup", 0), ("handler_address", 40),
                         ("vendor_handler", 40), ("vendor_metadata", 160),
                         ("vendor_slots", 192), ("vendor_guard", 392)):
        assert symbols[name] == base + offset, (name, symbols[name])
    assert symbols["code_end"] - base <= 160
    assert blob[symbols["code_end"] - base:] == bytes(416 - (symbols["code_end"] - base))
    assert struct.unpack_from("<I", blob, symbols["descriptor"] - base)[0] == 0x0006001D
    assert struct.unpack_from("<I", blob, symbols["stock_callback"] - base)[0] == 0x000D035D
    assert "dmb\tsy" in disassembly
    # DMB immediately precedes the sole producer head publication.
    assert bytes.fromhex("bff35f8f2360") in blob[:160]
    rom = (ROOT / "work/cyw43439-btrom-clean.bin").read_bytes()
    assert hashlib.sha256(rom).hexdigest() == ROM_SHA
    anchors = {
        0x6CFC0: "3b464a4669462046e047",  # lookup(cmd, descriptor, OCF, OGF)
        0x6CFD2: "bdf8040078b1e27ac01e8242",  # cmd[11] == descriptor.length - 3
        0x6D03C: "8df80870",  # free flag = 1
        0x6D044: "05f10802002202a9009b20469847",  # handler(cmd, &flag, event+8)
        0x6D052: "9df8080000281cbf2046aaf73dfd",  # free if flag stays nonzero
    }
    for offset, expected in anchors.items():
        assert rom[offset:offset + len(expected) // 2] == bytes.fromhex(expected), hex(offset)


def emulate(blob, symbols, base):
    try:
        import unicorn as un
        from unicorn import arm_const as arm
    except ImportError as error:
        raise SystemExit("Use: uv run --with unicorn python analysis/check-vendor-ingress.py --emulate") from error
    cpu = un.Uc(un.UC_ARCH_ARM, un.UC_MODE_THUMB | un.UC_MODE_MCLASS)
    cpu.mem_map(0x200000, 0x20000)
    cpu.mem_map(0xD0000, 0x1000)
    cpu.mem_write(base, blob)
    cmd, desc, free, response, stop, stack = 0x200100, 0x200180, 0x200190, 0x2001A0, 0x200200, 0x21FF00
    meta, slots = base + 160, base + 192
    regs = [getattr(arm, f"UC_ARM_REG_R{i}") for i in range(13)]
    preserved = [0x11220000 + i for i in range(8)]
    writes = []
    interrupts = False
    published = deque()
    interrupt_rng = random.Random(43439)
    interrupt_consumed = 0

    def word(address):
        return int.from_bytes(cpu.mem_read(address, 4), "little")

    def put(address, value):
        cpu.mem_write(address, struct.pack("<I", value & 0xFFFFFFFF))

    def code_hook(machine, address, _size, _user):
        nonlocal interrupt_consumed
        if address in (stop, 0xD035C):
            machine.emu_stop()
        elif interrupts and interrupt_rng.randrange(3) == 0:
            head, tail = word(meta), word(meta + 4)
            if head != tail:
                # Simulate a complete ISR at an instruction boundary, preserving registers.
                copied = bytes(cpu.mem_read(slots + tail * 40, 26))
                assert copied == published.popleft()
                put(meta + 4, (tail + 1) % 5)
                interrupt_consumed += 1
                put(meta + 16, interrupt_consumed)

    def write_hook(_machine, _access, address, size, value, _user):
        writes.append((address, size, value))
        if address == meta:
            old = word(meta)
            assert value == (old + 1) % 5
            assert bytes(cpu.mem_read(slots + old * 40, 26)) == bytes(cpu.mem_read(cmd + 12, 26))
            if interrupts:
                published.append(bytes(cpu.mem_read(cmd + 12, 26)))

    cpu.hook_add(un.UC_HOOK_CODE, code_hook)
    cpu.hook_add(un.UC_HOOK_MEM_WRITE, write_hook)

    def call(entry, arguments):
        writes.clear()
        for reg, value in zip(regs[:4], arguments):
            cpu.reg_write(reg, value)
        for reg, value in zip(regs[4:12], preserved):
            cpu.reg_write(reg, value)
        cpu.reg_write(arm.UC_ARM_REG_SP, stack)
        cpu.reg_write(arm.UC_ARM_REG_LR, stop | 1)
        cpu.reg_write(arm.UC_ARM_REG_APSR, 0xA8000000)
        cpu.emu_start(entry | 1, stop + 2, count=2000)
        assert cpu.reg_read(arm.UC_ARM_REG_PC) in (stop, 0xD035C)
        assert cpu.reg_read(arm.UC_ARM_REG_SP) == stack
        assert [cpu.reg_read(reg) for reg in regs[4:12]] == preserved

    for ocf, ogf in ((0xFF, 0x3F), (1, 4), (0xFE, 0x3F), (0xFF, 0), (0x3FF, 0x3F)):
        cpu.mem_write(desc, bytes([0xA5]) * 8)
        args = [cmd, desc, ocf, ogf]
        call(base, args)
        if (ocf, ogf) == (0xFF, 0x3F):
            assert word(desc) == symbols["vendor_handler"] | 1
            assert word(desc + 4) == 0x0006001D
        else:
            assert bytes(cpu.mem_read(desc, 8)) == bytes([0xA5]) * 8
            assert cpu.reg_read(arm.UC_ARM_REG_PC) == 0xD035C
            assert [cpu.reg_read(reg) for reg in regs[:4]] == args
            assert cpu.reg_read(arm.UC_ARM_REG_APSR) & 0xF8000000 == 0xA8000000

    def produce(payload, length=26):
        original = bytes(11) + bytes([length]) + payload
        cpu.mem_write(cmd, original)
        cpu.mem_write(free, b"\x01")
        cpu.mem_write(response, bytes([0xA5]) * 8)
        old_head = word(meta)
        call(symbols["vendor_handler"], [cmd, free, response, 0])
        assert bytes(cpu.mem_read(cmd, len(original))) == original
        assert bytes(cpu.mem_read(free, 1)) == b"\x01"
        assert bytes(cpu.mem_read(response, 5)) == bytes([0xA5]) * 5
        assert bytes(cpu.mem_read(response + 6, 2)) == bytes([0xA5]) * 2
        for address, size, _ in writes:
            assert (stack - 16 <= address and address + size <= stack
                    or address == response + 5 and size == 1
                    or address in (meta, meta + 8, meta + 12) and size == 4
                    or old_head <= 4 and slots + old_head * 40 <= address
                    and address + size <= slots + old_head * 40 + 26), (address, size)
        for slot in range(5):
            assert bytes(cpu.mem_read(slots + slot * 40 + 26, 14)) == bytes(14)
        assert bytes(cpu.mem_read(base + 392, 24)) == bytes(24)
        return cpu.mem_read(response + 5, 1)[0]

    for length in range(256):
        if length != 26:
            assert produce(bytes(26), length) == 0x12
    assert word(meta) == word(meta + 4) == word(meta + 8) == 0
    assert word(meta + 12) == 255
    for head, tail in ((5, 0), (0, 5), (0xFFFFFFFF, 0), (0, 0xFFFFFFFF)):
        put(meta, head)
        put(meta + 4, tail)
        assert produce(bytes(26)) == 0x12
        assert word(meta) == head and word(meta + 4) == tail
    for head in range(5):
        for tail in range(5):
            put(meta, head)
            put(meta + 4, tail)
            payload = bytes((head * 37 + tail * 13 + i) & 255 for i in range(26))
            full = (head + 1) % 5 == tail
            assert produce(payload) == (7 if full else 0)
            assert word(meta) == (head if full else (head + 1) % 5)

    cpu.mem_write(meta, bytes(232))
    queue = deque()
    rng = random.Random(43)
    accepted = rejected = consumed = 0
    for step in range(2000):
        if queue and rng.randrange(3) == 0:
            tail = word(meta + 4)
            # Model consumer's copy-before-release contract; never retain slot pointers.
            copied = bytes(cpu.mem_read(slots + tail * 40, 26))
            assert copied == queue.popleft()
            put(meta + 4, (tail + 1) % 5)
            consumed += 1
            put(meta + 16, consumed)
        else:
            payload = bytes((step + i * 17) & 255 for i in range(26))
            result = produce(payload)
            if len(queue) == 4:
                assert result == 7
                rejected += 1
            else:
                assert result == 0
                accepted += 1
                queue.append(payload)
            cpu.mem_write(cmd + 12, bytes([0xFF]) * 26)
        assert word(meta + 8) == accepted
        assert word(meta + 12) == rejected
        assert word(meta + 16) == consumed
    print(f"Thumb emulation: accepted={accepted} rejected={rejected} consumed={consumed}; bounds/ownership/order pass")
    cpu.mem_write(meta, bytes(232))
    interrupts = True
    for step in range(500):
        assert produce(bytes((step + i * 7) & 255 for i in range(26))) == 0
    assert word(meta + 8) == 500
    assert word(meta + 12) == 0
    assert interrupt_consumed + len(published) == 500
    print(f"Instruction-boundary ISR simulation: {interrupt_consumed} SDUs consumed without partial publication")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--emulate", action="store_true")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    reference = None
    with tempfile.TemporaryDirectory() as temporary:
        for base in (0x20A20C, 0x2184D8):
            blob, symbols, disassembly = build(Path(temporary), base)
            static_checks(blob, symbols, disassembly, base)
            if reference is not None:
                assert blob == reference, "image depends on load address"
            reference = blob
            if args.emulate:
                emulate(blob, symbols, base)
    if args.output:
        args.output.write_bytes(reference)
    print("vendor ingress: ROM ABI, position independence, 160-byte code bound, slots and guard pass")


if __name__ == "__main__":
    main()
