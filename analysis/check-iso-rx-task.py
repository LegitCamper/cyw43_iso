#!/usr/bin/env python3
"""Check one-shot task bridge and real emitter with stock transport/ring stubs."""
import argparse
from pathlib import Path
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def build(stream=False):
    with tempfile.TemporaryDirectory(prefix="iso-rx-task-") as directory:
        obj, elf, binary = [Path(directory) / name for name in ("task.o", "task.elf", "task.bin")]
        subprocess.run(["arm-none-eabi-as", "-mcpu=cortex-m3", "-mthumb", "-I", str(ROOT / "hardware")]
                       + (["--defsym", "RX_STREAM=1"] if stream else []) + [
                        str(ROOT / "hardware/iso_rx_task.S"), "-o", str(obj)], check=True)
        blobs = []
        for allocation in (0x209BAC, 0x217E78):
            base = allocation + 576
            subprocess.run(["arm-none-eabi-ld", f"-Ttext={base:#x}", "-e", "iso_rx_task",
                            str(obj), "-o", str(elf)], check=True)
            subprocess.run(["arm-none-eabi-objcopy", "-O", "binary", str(elf), str(binary)], check=True)
            symbols = subprocess.check_output(["arm-none-eabi-nm", "-n", str(elf)], text=True)
            values = {name: int(address, 16) for address, _, name in (line.split() for line in symbols.splitlines())}
            assert values["task_state"] == allocation + 736
            assert values["iso_rx_emit"] == allocation + 800
            assert values["rx_trace"] == allocation + 320
            blobs.append(binary.read_bytes())
        assert blobs[0] == blobs[1] and len(blobs[0]) <= 448
        assert blobs[0][160:172] == bytes(12)
        assert blobs[0][192:224] == bytes(32)
        if not stream:
            assert blobs[0][172:192] == bytes(20)
        return blobs[0]


def check(blob, stream=False):
    import unicorn as un
    from unicorn import arm_const as arm

    registers = [getattr(arm, f"UC_ARM_REG_R{i}") for i in range(13)]
    marker = bytes(range(26))
    for allocation in (0x209BAC, 0x217E78):
        for bad in (None, "wrap", "sequence-wrap", "count-wrap", "empty", "claimed", "header", "mode", "status", "event", "isr", "sent",
                    "bit2-counter", "neither-counter", "bit1-counter"):
            for primask in (0, 1):
                cpu = un.Uc(un.UC_ARCH_ARM, un.UC_MODE_THUMB | un.UC_MODE_MCLASS)
                cpu.mem_map(0, 0x100000)
                cpu.mem_map(0x200000, 0x20000)
                cpu.mem_write(allocation + 576, blob)
                trace, state, stop = allocation + 320, allocation + 736, 0xF0000
                def put(address, value):
                    cpu.mem_write(address, struct.pack("<I", value))
                put(trace, 0 if bad == "empty" else 1 if bad == "claimed" else 2)
                put(trace + 20, 2 if bad == "status" else 4)
                put(trace + 24, 0x1F001F00 if bad == "header" else 0x1A001A00)
                put(trace + 44, 0 if bad == "mode" else 1)
                before = [65535 if bad == "wrap" else 0, 3, 7]
                after = [(before[0] + 1) & 0xFFFF, 3, 7]
                for index, name in enumerate(("bit2-counter", "neither-counter", "bit1-counter")):
                    if bad == name:
                        after[index] += 1
                cpu.mem_write(trace + 28, struct.pack("<6H", *before, *after))
                cpu.mem_write(trace + 48, marker + bytes([0xDD]) * 6)
                count = 1 if bad == "sent" else 65535 if stream and bad == "sequence-wrap" else 0xFFFFFFFF if stream and bad == "count-wrap" else 0
                put(state, count)
                cpu.mem_write(allocation + 768, bytes([0x5A]) * 20)
                record = bytes(cpu.mem_read(trace, 80))
                incoming = [0x11000000 + i * 0x101 for i in range(13)]
                incoming[:2] = [0x206000, 2 if bad == "event" else 3]
                calls = []
                ring_result = 1

                def code(machine, address, _size, _user):
                    if address == stop:
                        machine.emu_stop()
                        return
                    if address not in (0x44104, 0x5FC54, 0x5F87A):
                        return
                    assert machine.reg_read(arm.UC_ARM_REG_SP) % 8 == 0
                    if address == 0x44104:
                        calls.append("stock")
                        for index in (0, 1, 2, 3, 12):
                            assert machine.reg_read(registers[index]) == incoming[index]
                        assert machine.reg_read(arm.UC_ARM_REG_APSR) & 0xF8000000 == 0xA8000000
                        result = 0xCA000000
                    elif address == 0x5FC54:
                        assert calls == ["stock"]
                        assert machine.reg_read(registers[0]) == 5
                        assert machine.reg_read(registers[2]) == 34
                        packet = bytes(machine.mem_read(machine.reg_read(registers[1]), 34))
                        assert packet == struct.pack("<4H", 0x2042, 30, count & 0xFFFF if stream else 0, 26) + marker
                        calls.append("ring")
                        result = ring_result
                    else:
                        assert calls == ["stock", "ring"] and ring_result == 0
                        calls.append("notify")
                        result = 0xCA000000
                    for index in (0, 1, 2, 3, 12):
                        machine.reg_write(registers[index], 0xCA000000 + index)
                    machine.reg_write(registers[0], result)
                    machine.reg_write(arm.UC_ARM_REG_APSR, 0x58000000)
                    machine.reg_write(arm.UC_ARM_REG_PC, machine.reg_read(arm.UC_ARM_REG_LR))

                cpu.hook_add(un.UC_HOOK_CODE, code)
                def invoke():
                    calls.clear()
                    for register, value in zip(registers, incoming):
                        cpu.reg_write(register, value)
                    cpu.reg_write(arm.UC_ARM_REG_SP, 0x21D000)
                    cpu.reg_write(arm.UC_ARM_REG_LR, stop | 1)
                    cpu.reg_write(arm.UC_ARM_REG_APSR, 0xA8000000)
                    cpu.reg_write(arm.UC_ARM_REG_IPSR, 26 if bad == "isr" else 0)
                    cpu.reg_write(arm.UC_ARM_REG_PRIMASK, primask)
                    cpu.emu_start((allocation + 576) | 1, 0, count=10000)
                    assert cpu.reg_read(arm.UC_ARM_REG_PC) == stop
                    assert cpu.reg_read(arm.UC_ARM_REG_SP) == 0x21D000
                    assert cpu.reg_read(arm.UC_ARM_REG_PRIMASK) == primask
                    assert [cpu.reg_read(register) for register in registers[4:12]] == incoming[4:12]
                    for index in (0, 1, 2, 3, 12):
                        assert cpu.reg_read(registers[index]) == 0xCA000000 + index
                    assert cpu.reg_read(arm.UC_ARM_REG_APSR) & 0xF8000000 == 0x58000000
                    assert bytes(cpu.mem_read(trace + 4, 76)) == record[4:]
                    if not stream:
                        assert bytes(cpu.mem_read(trace, 4)) == record[:4]
                    assert bytes(cpu.mem_read(allocation + 768, 20)) == bytes([0x5A]) * 20
                    return struct.unpack("<2I", cpu.mem_read(state, 8))

                if bad in (None, "wrap", "sequence-wrap", "count-wrap") or stream and bad == "sent":
                    assert invoke() == (count, 1) and calls == ["stock", "ring"]
                    ring_result = 0
                    final = (count + 1) & 0xFFFFFFFF if stream else 1
                    assert invoke() == (final, 0) and calls == ["stock", "ring", "notify"]
                    assert invoke() == (final, 0) and calls == ["stock"]
                    if stream:
                        assert struct.unpack("<I", cpu.mem_read(trace, 4))[0] == 0
                        assert struct.unpack("<I", cpu.mem_read(state + 8, 4))[0] == 0
                elif stream and bad in ("header", "mode", "status", "bit2-counter", "neither-counter", "bit1-counter"):
                    assert invoke() == (0, 2) and calls == ["stock"]
                    assert struct.unpack("<I", cpu.mem_read(trace, 4))[0] == 0
                    assert struct.unpack("<I", cpu.mem_read(state + 8, 4))[0] == 1
                else:
                    assert invoke() == (1 if bad == "sent" else 0, 0)
                    assert calls == ["stock"]
    print(f"ISO RX task: {len(blob)} PIC overlay bytes; guards/counter wrap/mismatch, stock-first ABI/flags, ring-full retry, single delivery and record preservation pass; external transport/ring calls stubbed.")


def check_interleaving(resident):
    import unicorn as un
    from unicorn import arm_const as arm

    rom = (ROOT / "work/cyw43439-btrom-clean.bin").read_bytes()
    registers = [getattr(arm, f"UC_ARM_REG_R{i}") for i in range(13)]
    first, second = bytes(range(26)), bytes(range(128, 154))
    for allocation in (0x209BAC, 0x217E78):
        for point in ("copy", "ring-full", "ring-success", "between", "after-rearm"):
            trace, state, event, params = allocation + 320, allocation + 736, 0x209788, 0x2097B8
            def machine():
                cpu = un.Uc(un.UC_ARCH_ARM, un.UC_MODE_THUMB | un.UC_MODE_MCLASS)
                cpu.mem_map(0, 0x100000)
                cpu.mem_write(0, rom)
                cpu.mem_map(0x200000, 0x20000)
                cpu.mem_map(0x318000, 0x1000)
                cpu.mem_map(0x370000, 0x1000)
                return cpu
            cpu = machine()
            cpu.mem_write(allocation, resident)
            def put(target, address, value):
                target.mem_write(address, struct.pack("<I", value))
            def word(target, address):
                return struct.unpack("<I", target.mem_read(address, 4))[0]
            put(cpu, event + 28, params)
            cpu.mem_write(event + 15, b"\x01")
            cpu.mem_write(params + 6, b"\x01\x02")
            cpu.mem_write(params + 10, struct.pack("<3H", 1, 0, 0))
            put(cpu, 0x201F1C, 0x206000)
            put(cpu, trace, 2)
            put(cpu, trace + 20, 4)
            put(cpu, trace + 24, 0x1A001A00)
            put(cpu, trace + 44, 1)
            cpu.mem_write(trace + 28, struct.pack("<6H", 0, 0, 0, 1, 0, 0))
            cpu.mem_write(trace + 48, first + bytes(6))
            put(cpu, state, 65535)
            ring_result, injected, pending = (1 if point == "ring-full" else 0), False, False
            packets, log = [], []
            expected_sequence, expected_payload, expected_count = 65535, first, 65535

            def finish_stub(target, result=0xCA000000):
                for index in (0, 1, 2, 3, 12):
                    target.reg_write(registers[index], 0xCA000000 + index)
                target.reg_write(registers[0], result)
                target.reg_write(arm.UC_ARM_REG_APSR, 0x58000000)
                target.reg_write(arm.UC_ARM_REG_PC, target.reg_read(arm.UC_ARM_REG_LR))

            def interrupt(header=0x1A001A00):
                # Two CPU instances share copied SRAM at an instruction boundary.
                # The IRQ executes the actual resident hook; task registers stay suspended.
                irq = machine()
                irq.mem_write(0x200000, bytes(cpu.mem_read(0x200000, 0x20000)))
                put(irq, 0x318BAC, 6)  # Both bits: stock bit2 takes precedence.
                put(irq, 0x318B98, header)
                irq.mem_write(0x370A00, second + bytes(6))
                before = bytes(irq.mem_read(trace, 80))
                original_state = word(irq, trace)
                incoming = [0x22000000 + index for index in range(13)]
                incoming[0] = event
                for register, value in zip(registers, incoming):
                    irq.reg_write(register, value)
                irq.reg_write(arm.UC_ARM_REG_SP, 0x21B000)
                irq.reg_write(arm.UC_ARM_REG_LR, 0xF1001)
                irq.reg_write(arm.UC_ARM_REG_IPSR, 26)
                irq.reg_write(arm.UC_ARM_REG_APSR, 0xA8000000)
                irq_calls = []
                def irq_code(target, address, _size, _user):
                    if address == 0xF1000:
                        target.emu_stop()
                    elif address == 0x7143E:
                        irq_calls.append("stock")
                        counter = struct.unpack("<H", target.mem_read(params + 10, 2))[0]
                        target.mem_write(params + 10, struct.pack("<H", (counter + 1) & 0xFFFF))
                        target.mem_write(event + 44, bytes([206]))
                        finish_stub(target)
                    elif address == 0x9060:
                        assert irq_calls == ["stock"]
                        assert [target.reg_read(registers[index]) for index in range(3)] == [0x2060B4, 1, 0]
                        assert word(target, trace) == 2
                        irq_calls.append("wake")
                        finish_stub(target)
                irq.hook_add(un.UC_HOOK_CODE, irq_code)
                irq.emu_start(allocation | 1, 0, count=10000)
                assert irq.reg_read(arm.UC_ARM_REG_PC) == 0xF1000
                assert irq.reg_read(arm.UC_ARM_REG_SP) == 0x21B000
                assert [irq.reg_read(register) for register in registers[4:12]] == incoming[4:12]
                captured = original_state == 0 and header == 0x1A001A00
                if captured:
                    assert word(irq, trace) == 2 and bytes(irq.mem_read(trace + 48, 26)) == second
                else:
                    assert bytes(irq.mem_read(trace, 80)) == before, "IRQ overwrote owned mailbox"
                assert irq_calls == (["stock", "wake"] if captured or original_state == 2 else ["stock"])
                cpu.mem_write(0x200000, bytes(irq.mem_read(0x200000, 0x20000)))
                log.append("irq")

            def code(target, address, _size, _user):
                nonlocal injected, pending
                if pending:
                    pending = False
                    injected = True
                    interrupt()
                if point == "copy" and not injected and allocation + 800 <= address < allocation + 912 and target.reg_read(registers[1]) == trace + 49:
                    injected = True
                    interrupt()
                if address == 0xF0000:
                    target.emu_stop()
                elif address == 0x44104:
                    log.append("stock")
                    finish_stub(target)
                elif address == 0x5FC54:
                    if not injected and point in ("ring-full", "ring-success"):
                        injected = True
                        interrupt()
                    packet = bytes(target.mem_read(target.reg_read(registers[1]), 34))
                    assert packet == struct.pack("<4H", 0x2042, 30, expected_sequence, 26) + expected_payload
                    packets.append(packet)
                    log.append("ring")
                    finish_stub(target, ring_result)
                elif address == 0x5F87A:
                    assert ring_result == 0
                    log.append("notify")
                    finish_stub(target)

            def write(target, _access, address, _size, value, _user):
                nonlocal pending
                if address == state:
                    assert log[-1] == "notify", "count published before copy/notify"
                    if point == "between" and not injected:
                        pending = True
                if address == trace and value == 0:
                    assert word(target, state) == (expected_count + 1) & 0xFFFFFFFF
                    if point == "after-rearm" and not injected:
                        pending = True
            cpu.hook_add(un.UC_HOOK_CODE, code)
            cpu.hook_add(un.UC_HOOK_MEM_WRITE, write, begin=state, end=state + 3)
            cpu.hook_add(un.UC_HOOK_MEM_WRITE, write, begin=trace, end=trace + 3)
            def invoke():
                incoming = [0x11000000 + index for index in range(13)]
                incoming[:2] = [0x206000, 1]
                for register, value in zip(registers, incoming):
                    cpu.reg_write(register, value)
                cpu.reg_write(arm.UC_ARM_REG_SP, 0x21D000)
                cpu.reg_write(arm.UC_ARM_REG_LR, 0xF0001)
                cpu.reg_write(arm.UC_ARM_REG_APSR, 0xA8000000)
                cpu.emu_start((allocation + 576) | 1, 0, count=10000)
                assert cpu.reg_read(arm.UC_ARM_REG_PC) == 0xF0000
                assert cpu.reg_read(arm.UC_ARM_REG_SP) == 0x21D000
                assert [cpu.reg_read(register) for register in registers[4:12]] == incoming[4:12]
                assert [cpu.reg_read(registers[index]) for index in (0, 1, 2, 3, 12)] == [0xCA000000 + index for index in (0, 1, 2, 3, 12)]
                assert cpu.reg_read(arm.UC_ARM_REG_APSR) & 0xF8000000 == 0x58000000
            invoke()
            assert injected
            if point == "ring-full":
                assert word(cpu, trace) == 2 and word(cpu, state) == 65535
                assert word(cpu, state + 4) == 1
                ring_result = 0
                invoke()
                assert packets[0] == packets[1], "retry changed sequence/payload"
            assert word(cpu, state) == 65536
            if point == "after-rearm":
                assert word(cpu, trace) == 2, "task erased packet arriving after rearm"
                expected_sequence, expected_payload, expected_count = 0, second, 65536
                # Count is u32, while HCI sequence has wrapped to zero.
                invoke()
                assert word(cpu, state) == 65537 and word(cpu, trace) == 0
            else:
                assert word(cpu, trace) == 0
                interrupt(header=0x1F001F00)
                assert word(cpu, trace) == 0, "unsupported header poisoned empty mailbox"
                interrupt()
                assert word(cpu, trace) == 2, "valid packet after wrong header not captured"
    print("RX stream interleavings pass: IRQ during copy/full/success/count publication/rearm, immutable slot, retry identity, next-slot retention and accepted sequence wrap; ROM task/ring/wake helpers stubbed.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    blob = build()
    check(blob)
    if args.output:
        args.output.write_bytes(blob)
