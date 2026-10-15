#!/usr/bin/env python3
"""Build PIC HCI credit helpers; optionally execute Thumb and ROM allocation ABI."""
import argparse
import hashlib
from pathlib import Path
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def build(directory, base):
    obj, elf, binary = (directory / name for name in ("credits.o", "credits.elf", "credits.bin"))
    subprocess.run(["arm-none-eabi-as", "-mcpu=cortex-m3", "-mthumb",
                    str(ROOT / "hardware/iso_credits.S"), "-o", str(obj)], check=True)
    subprocess.run(["arm-none-eabi-ld", f"-Ttext={base:#x}", "-e", "iso_credit_lookup",
                    str(obj), "-o", str(elf)], check=True)
    subprocess.run(["arm-none-eabi-objcopy", "-O", "binary", str(elf), str(binary)], check=True)
    symbols = {name: int(value, 16) for value, _, name in
               (line.split() for line in subprocess.check_output(
                   ["arm-none-eabi-nm", "-n", str(elf)], text=True).splitlines())}
    return binary.read_bytes(), symbols


def emulate(blob, symbols, base, rom):
    import unicorn as un
    from unicorn import arm_const as arm
    cpu = un.Uc(un.UC_ARCH_ARM, un.UC_MODE_THUMB | un.UC_MODE_MCLASS)
    cpu.mem_map(0, (len(rom) + 4095) & ~4095)
    cpu.mem_write(0, rom)
    cpu.mem_map(0xD0000, 0x1000)
    cpu.mem_map(0x200000, 0x20000)
    cpu.mem_write(base, blob)
    command, desc, free, response = 0x200100, 0x200180, 0x200190, 0x2001A0
    event, stop, stack = 0x200300, 0x200400, 0x21FF00
    alloc_ok, allocations, sent = True, [], []
    regs = [getattr(arm, f"UC_ARM_REG_R{i}") for i in range(13)]
    saved = [0xAABB0000 + i for i in range(8)]

    def ret(value=None):
        if value is not None:
            cpu.reg_write(regs[0], value)
        cpu.reg_write(arm.UC_ARM_REG_PC, cpu.reg_read(arm.UC_ARM_REG_LR))

    def hook(_cpu, address, _size, _user):
        if address in (stop, 0xD035C):
            cpu.emu_stop()
        elif address == 0x179DC:
            assert cpu.reg_read(regs[0]) == 15  # 8 object bytes + 7 HCI bytes
            allocations.append(alloc_ok)
            ret(event if alloc_ok else 0)
        elif address == 0x8216A:
            dest, length = [cpu.reg_read(reg) for reg in regs[:2]]
            assert (dest, length) == (event, 15)
            cpu.mem_write(dest, bytes(length))
            ret()
        elif address == 0x40E52:
            assert cpu.reg_read(regs[0]) == event
            raw = bytes(cpu.mem_read(event, 15))
            assert raw[:8] == bytes.fromhex("0000000041700000")
            assert raw[8:13] == bytes.fromhex("1305014200")
            sent.append(raw[8:])
            ret(0xDEADBEEF)  # ROM send has no boolean success contract.

    cpu.hook_add(un.UC_HOOK_CODE, hook)

    def call(name, args):
        for reg, value in zip(regs[:4], args):
            cpu.reg_write(reg, value)
        for reg, value in zip(regs[4:12], saved):
            cpu.reg_write(reg, value)
        cpu.reg_write(arm.UC_ARM_REG_SP, stack)
        cpu.reg_write(arm.UC_ARM_REG_LR, stop | 1)
        cpu.reg_write(arm.UC_ARM_REG_APSR, 0xA8000000)
        cpu.emu_start(symbols[name] | 1, stop + 2, count=1000)
        assert cpu.reg_read(arm.UC_ARM_REG_PC) in (stop, 0xD035C)
        assert cpu.reg_read(arm.UC_ARM_REG_SP) == stack
        assert [cpu.reg_read(reg) for reg in regs[4:12]] == saved
        return cpu.reg_read(regs[0])

    for ocf, ogf in ((0x60, 8), (2, 8), (0x60, 7), (0xFF, 0x3F)):
        cpu.mem_write(desc, bytes([0xA5]) * 8)
        args = [command, desc, ocf, ogf]
        call("iso_credit_lookup", args)
        if (ocf, ogf) == (0x60, 8):
            assert bytes(cpu.mem_read(desc, 8)) == struct.pack(
                "<II", symbols["iso_buffer_size_v2"] | 1, 0x000C0003)
        else:
            assert bytes(cpu.mem_read(desc, 8)) == bytes([0xA5]) * 8
            assert [cpu.reg_read(reg) for reg in regs[:4]] == args
            assert cpu.reg_read(arm.UC_ARM_REG_APSR) & 0xF8000000 == 0xA8000000

    for acl_length, acl_count in ((0, 0), (1021, 8), (65535, 255)):
        cpu.mem_write(0x2025B1, struct.pack("<H", acl_length) + bytes(6) + bytes([acl_count]))
        for length in range(256):
            original = bytes(11) + bytes([length])
            cpu.mem_write(command, original)
            cpu.mem_write(free, b"\x01")
            cpu.mem_write(response, bytes.fromhex("0e0a016020") + bytes([0xA5]) * 9)
            call("iso_buffer_size_v2", [command, free, response, 0])
            expected = (struct.pack("<BHBHB", 0, acl_length, acl_count, 30, 4)
                        if length == 0 else b"\x12" + bytes(6))
            assert bytes(cpu.mem_read(response, 14)) == bytes.fromhex("0e0a016020") + expected + b"\xa5\xa5"
            assert bytes(cpu.mem_read(command, 12)) == original
            assert bytes(cpu.mem_read(free, 1)) == b"\x01"

    for count in (0, 5, 0xFFFF, 0x10000, 0xFFFFFFFF):
        assert call("iso_emit_completed", [count, 0, 0, 0]) == 0
        assert not allocations and not sent
    for count in range(1, 5):
        cpu.mem_write(event, bytes([0xA5]) * 19)
        alloc_ok = False
        before = len(sent)
        assert call("iso_emit_completed", [count, 0, 0, 0]) == 0
        assert len(sent) == before
        assert bytes(cpu.mem_read(event, 19)) == bytes([0xA5]) * 19
        alloc_ok = True
        assert call("iso_emit_completed", [count, 0, 0, 0]) == 1
        assert len(sent) == before + 1
        assert sent[-1] == bytes.fromhex("1305014200") + struct.pack("<H", count)
        assert bytes(cpu.mem_read(event + 15, 4)) == bytes([0xA5]) * 4
    print(f"{base:#x}: lookup, stock ACL fields, lengths, allocator failure/retry, credits 1..4 pass")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--emulate", action="store_true")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    rom = (ROOT / "work/cyw43439-btrom-clean.bin").read_bytes()
    assert hashlib.sha256(rom).hexdigest() == "d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038"
    anchors = {
        0x51744: "ed480188d180007a10727047",  # stock ACL buffer fields
        0x51AFC: "b1252000",
        0x877FC: "4517050000000900",  # LE Read Buffer Size v1 table descriptor
        0x41214: "2de9f04100f1080816460f464046d6f7dbfb040009d0",
        0x41234: "47f2410060602f706e702046bde8f081",
        0x40E72: "087a13282bd0",  # Number Of Completed Packets bypasses event mask
        0x412D6: "00b583b013228df8002005228df8012001228df80220adf80300adf80510",
        0x6D052: "9df8080000281cbf2046aaf73dfd",  # command ownership/free flag
    }
    for address, encoded in anchors.items():
        assert rom[address:address + len(encoded) // 2] == bytes.fromhex(encoded), hex(address)
    previous = None
    with tempfile.TemporaryDirectory(prefix="cyw43-credits-") as temp:
        for base in (0x20A6AC, 0x218000):
            blob, symbols = build(Path(temp), base)
            assert len(blob) == 148
            assert symbols["iso_buffer_size_v2"] == base + 40
            assert symbols["iso_emit_completed"] == base + 88
            assert symbols["iso_credit_previous_lookup"] == base + 36
            assert previous is None or previous == blob
            previous = blob
            if args.emulate:
                emulate(blob, symbols, base, rom)
    if args.output:
        args.output.write_bytes(previous)
    print("HCI credit helpers: 148-byte PIC image and exact ROM ABI anchors pass")


if __name__ == "__main__":
    main()
