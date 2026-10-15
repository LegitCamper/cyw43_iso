#!/usr/bin/env python3
"""Check embedded Thumb wrapper against assembler and clean-ROM callback ABI."""

from pathlib import Path
import argparse
import re
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]


def check_capture(path, phase=False, timing=False, sram=False):
    text = path.read_text()
    for line in ("RUNTIME_PROBE execution=pass", "RUNTIME_PROBE callback-restored=1",
                 "RUNTIME_PROBE restored complete=1 stock-version=1"):
        assert line in text, f"missing successful checkpoint: {line}"
    assert re.search(r"RUNTIME_PROBE count=1 .*ocf=00000001 ogf=00000004", text)
    if sram:
        for line in ("RUNTIME_PROBE SRAM_PROBE-allocated complete=1 stock-version=1",
                     "RUNTIME_PROBE SRAM_PROBE-allocation-repeat complete=1 stock-version=1",
                     "RUNTIME_PROBE SRAM_PROBE-allocation-restored complete=1 stock-version=1",
                     "RUNTIME_PROBE SRAM_PROBE-executed complete=1 stock-version=1",
                     "RUNTIME_PROBE SRAM_PROBE-restored complete=1 stock-version=1",
                     "SRAM_PROBE callback-restored=1", "SRAM_PROBE scratch-and-arena-retained-until-reset=1",
                     "SRAM_PROBE result=pass"):
            assert line in text, f"missing successful checkpoint: {line}"
        before = re.search(r"SRAM_PROBE bounds initialized=1 cursor=([0-9a-f]{8}) end=([0-9a-f]{8}) size=512", text)
        after = re.search(r"SRAM_PROBE allocation=([0-9a-f]{8}) cursor-after=([0-9a-f]{8}) end-after=([0-9a-f]{8})", text)
        assert before and after
        cursor, end = (int(value, 16) for value in before.groups())
        allocated, cursor_after, end_after = (int(value, 16) for value in after.groups())
        assert 0x209bac <= cursor <= 0x21de00 and cursor % 4 == 0 and end % 4 == 0
        assert cursor + 512 < end <= 0x220000 and end_after == end
        assert allocated == cursor and cursor_after == cursor + 512
        assert re.search(r"SRAM_PROBE count=1 .*ocf=00000001 ogf=00000004", text)
        print(f"SRAM allocation/execution and restoration checkpoints pass: {path}")
        return
    if timing:
        for line in ("PHASE_TIMING global-restored=1", "PHASE_TIMING scratch-retained-until-reset=1",
                     "RUNTIME_PROBE PHASE_TIMING-restored complete=1 stock-version=1",
                     "PHASE_TIMING connection status=00", "PHASE_TIMING disconnection status=00"):
            assert line in text, f"missing successful checkpoint: {line}"
        assert not re.search(r"PHASE_TIMING (?:result=failed|shutdown-uncertain|alias-guard-failed)", text)
        table = (ROOT / "work/cyw43439-btrom-clean.bin").read_bytes()[0x8af40:0x8af6c]
        assert f"PHASE_TIMING stock-table={table.hex()}" in text
        samples = [int(value) for value in re.findall(r"PHASE_TIMING delta27=(\d+) time=\d+", text)]
        assert all(0 <= value <= 0x07ffffff for value in samples)
        if "table-installed mode=control" in text:
            assert not samples
            assert "PHASE_TIMING execution=not-instrumented" in text and "PHASE_TIMING result=control-pass" in text
        else:
            assert "table-installed mode=instrumented" in text and samples
            assert "PHASE_TIMING execution=pass" in text and "PHASE_TIMING result=pass" in text
            print(f"phase-2 delta27 values: {sorted(set(samples))}")
        print(f"timing capture checkpoints pass (atomic stored-clock delta only): {path}")
        return
    if phase:
        assert "PHASE_PROBE result=failed" not in text and "PHASE_PROBE shutdown-uncertain" not in text
        for line in ("PHASE_PROBE global-restored=1", "PHASE_PROBE scratch-retained-until-reset=1",
                     "RUNTIME_PROBE PHASE_PROBE-restored complete=1 stock-version=1",
                     "PHASE_PROBE connection status=00", "PHASE_PROBE disconnection status=00"):
            assert line in text, f"missing successful checkpoint: {line}"
        table = (ROOT / "work/cyw43439-btrom-clean.bin").read_bytes()[0x8af40:0x8af6c]
        assert f"PHASE_PROBE stock-table={table.hex()}" in text
        samples = re.findall(r"PHASE_PROBE phase0=(\d+) phase1=(\d+) phase2=(\d+) other=(\d+) event=([0-9a-f]{8}) time=(\d+)", text)
        assert samples, "missing phase samples"
        counts = [tuple(map(int, sample[:4])) for sample in samples]
        assert all(all(b >= a for a, b in zip(left, right)) for left, right in zip(counts, counts[1:])), "counters decreased"
        if "table-installed mode=control" in text:
            assert all(values == (0, 0, 0, 0) for values in counts)
            assert all(int(sample[4], 16) == 0 for sample in samples)
            assert "PHASE_PROBE execution=not-instrumented" in text and "PHASE_PROBE result=control-pass" in text
        else:
            assert "table-installed mode=instrumented" in text
            assert all(counts[-1][i] > 0 for i in range(3)), "phases 0/1/2 not all observed"
            assert "PHASE_PROBE execution=pass" in text and "PHASE_PROBE result=pass" in text
            events = [int(sample[4], 16) for sample in samples if int(sample[4], 16)]
            assert events and all(0x200000 <= event <= 0x21fe60 and event % 4 == 0 for event in events)
        print(f"phase capture checkpoints pass (host sampling only): {path}")
        return
    for line in ("STATE7_PROBE prologue-restored=1", "STATE7_PROBE scratch-restored=1",
                 "RUNTIME_PROBE state7-restored complete=1 stock-version=1"):
        assert line in text, f"missing successful checkpoint: {line}"
    assert "STATE7_PROBE connection status=00" in text
    assert "STATE7_PROBE disconnection status=00" in text
    counts = [int(value) for value in re.findall(r"STATE7_PROBE count=(\d+)", text)]
    assert counts, "missing state-7 samples"
    if "mode=control" in text:
        assert counts == [0] and "STATE7_PROBE result=control-pass" in text
        assert "STATE7_PROBE execution=not-instrumented" in text
    else:
        assert "mode=instrumented" in text and max(counts) > 0
        assert "STATE7_PROBE result=pass" in text
        match = re.search(r"STATE7_PROBE post-block address=([0-9a-f]+) bytes=([0-9a-f]+)", text)
        assert match, "missing post-entry block sample"
        entry = re.search(r"STATE7_PROBE count=[1-9]\d* r0=([0-9a-f]+)", text)
        assert entry and int(match[1], 16) == int(entry[1], 16) + 0x154
        block = bytes.fromhex(match[2])
        assert len(block) == 64 and block[:2] == b"\x01\x02"
        def half(offset):
            start = offset - 0x154
            return int.from_bytes(block[start:start + 2], "little")
        for output, left, right in ((0x170, 0x160, 0x16c), (0x174, 0x164, 0x168),
                                    (0x17c, 0x166, 0x16a), (0x178, 0x162, 0x16e)):
            assert half(output) == min(half(left), half(right)), hex(output)
        assert half(0x17e) == max(half(0x17c), 2704)
        assert half(0x176) == half(0x17c) and half(0x172) == half(0x178)
    print(f"capture checkpoints pass: {path}")


def prepare(phase=False, timing=False, sram=False):
    stock = ROOT / "work/sdk0065.bin"
    command = ["cargo", "run", "--quiet", "--"]
    subprocess.run(command + ["verify", str(stock)], cwd=ROOT, check=True)
    data = bytearray(stock.read_bytes())
    capacity = 112 if timing or phase or sram else 56
    count_offset = data[0] + 1
    offset, base = count_offset + 1, 0
    if not 1 <= data[count_offset] <= 253:
        raise ValueError("expected EOF and room for two appended records")
    for index in range(data[count_offset]):
        length, kind = data[offset], data[offset + 3]
        address = base + int.from_bytes(data[offset + 1:offset + 3], "big")
        payload = data[offset + 4:offset + 4 + length]
        if kind == 0 and address < 0xD0F90 + capacity and address + length > 0xD0F90:
            raise ValueError("append overlaps loaded firmware")
        if kind in (2, 4):
            base = int.from_bytes(payload, "big") << (4 if kind == 2 else 16)
        if (kind == 1) != (index == data[count_offset] - 1):
            raise ValueError("expected exactly one final EOF record")
        if kind == 1:
            eof_offset = offset
        offset += 4 + length
    if offset != len(data) or data[eof_offset:] != b"\x00\x00\x00\x01":
        raise ValueError("unexpected EOF encoding")
    data[count_offset] += 2
    data[eof_offset:eof_offset] = bytes.fromhex("02000004000d") + bytes([capacity, 0x0f, 0x90, 0]) + bytes(capacity)
    output = ROOT / ("work/sram-exec-probe/zero-append.bin" if sram else
                     "work/event-phase-timing-compact/zero-append.bin" if timing else
                     "work/event-phase-probe/zero-append.bin" if phase else "work/runtime-hook-probe/zero-append.bin")
    header = output.parent / "custom-fw/cyw43_btfw_43439.h"
    header.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(data)
    subprocess.run(command + ["verify", str(output)], cwd=ROOT, check=True)
    subprocess.run(command + ["c-header", str(output), str(header)], cwd=ROOT, check=True)
    print(f"prepared {output}\nprepared {header}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepare", action="store_true", help="append zeroed probe space to stock firmware")
    parser.add_argument("--phase", action="store_true", help="prepare/check scheduler phase mode with 112-byte reservation")
    parser.add_argument("--timing", action="store_true", help="prepare/check atomic phase-2 delta with 112-byte reservation")
    parser.add_argument("--sram", action="store_true", help="prepare/check bounded resident allocation and SRAM callback execution")
    parser.add_argument("--capture", type=Path, help="check retained state-7 capture and timing equations")
    arguments = parser.parse_args()
    source = (ROOT / "hardware/picow_runtime_probe.c").read_text()
    def embedded_bytes(name):
        initializer = re.search(r"uint8_t " + name + r"\[\] = \{(.*?)\};", source, re.S).group(1)
        return bytes(int(value.strip(), 0) for value in initializer.split(",") if value.strip())

    with tempfile.TemporaryDirectory() as directory:
        temporary = Path(directory)
        obj, elf, binary = (temporary / name for name in ("hook.o", "hook.elf", "hook.bin"))
        for name, size, target in (("wrapper", 52, 0xD035D), ("state7_wrapper", 56, 0xD0611)):
            flags = ["--defsym", "STATE7_PROBE=1"] if name == "state7_wrapper" else []
            subprocess.run(["arm-none-eabi-as", "-mcpu=cortex-m3", "-mthumb", *flags,
                            str(ROOT / "hardware/runtime_hook.S"), "-o", str(obj)], check=True)
            subprocess.run(["arm-none-eabi-ld", "-Ttext=0xd0f90", f"--defsym=stock_hook={target:#x}",
                            "-e", "hook", str(obj), "-o", str(elf)], check=True)
            subprocess.run(["arm-none-eabi-objcopy", "-O", "binary", str(elf), str(binary)], check=True)
            embedded = embedded_bytes(name)
            assert binary.read_bytes() == embedded, f"{name} differs from assembled source"
            assert len(embedded) == size
            assert int.from_bytes(embedded[size-24:size-20], "little") == 0xD0F90 + size - 20
            assert embedded[-20:] == bytes(20), "telemetry must start zero"
        subprocess.run(["arm-none-eabi-as", "-mcpu=cortex-m3", "-mthumb", "--defsym", "EVENT_PHASE_PROBE=1",
                        str(ROOT / "hardware/runtime_hook.S"), "-o", str(obj)], check=True)
        subprocess.run(["arm-none-eabi-ld", "-Ttext=0xd0f90", "--defsym=stock_hook=0x4917f",
                        "-e", "hook", str(obj), "-o", str(elf)], check=True)
        subprocess.run(["arm-none-eabi-objcopy", "-O", "binary", str(elf), str(binary)], check=True)
        phase_wrapper = embedded_bytes("phase_wrapper")
        assert len(phase_wrapper) == 48 and binary.read_bytes() == phase_wrapper + bytes(20)
        assert int.from_bytes(phase_wrapper[40:44], "little") == 0xd0fc0
        assert 0xd0f90 + len(phase_wrapper) + 20 == 0xd0fd4
        assert 0xd0fd4 + 44 == 0xd1000
        subprocess.run(["arm-none-eabi-as", "-mcpu=cortex-m3", "-mthumb", "--defsym", "EVENT_PHASE_TIMING=1",
                        str(ROOT / "hardware/runtime_hook.S"), "-o", str(obj)], check=True)
        subprocess.run(["arm-none-eabi-ld", "-Ttext=0xd0f90", "--defsym=stock_hook=0x4917f",
                        "-e", "hook", str(obj), "-o", str(elf)], check=True)
        subprocess.run(["arm-none-eabi-objcopy", "-O", "binary", str(elf), str(binary)], check=True)
        timing_wrapper = embedded_bytes("timing_wrapper")
        assert len(timing_wrapper) == 64 and binary.read_bytes() == timing_wrapper + bytes(20)
        assert int.from_bytes(timing_wrapper[48:52], "little") == 0xd0fd0
        assert 0xd0f90 + 64 == 0xd0fd0 and 0xd0fd0 + 4 + 44 == 0xd1000
        disassembly = subprocess.check_output(["arm-none-eabi-objdump", "-d", str(elf)], text=True)
        assert re.search(r"bl\s+4917e", disassembly)
        assert re.search(r"ubfx\s+r6, r6, #0, #27", disassembly)
        assert re.search(r"str\s+r6, \[r7, #0\]", disassembly)
        subprocess.run(["arm-none-eabi-as", "-mcpu=cortex-m3", "-mthumb", "--defsym", "SRAM_EXEC_ALLOC=1",
                        str(ROOT / "hardware/runtime_hook.S"), "-o", str(obj)], check=True)
        subprocess.run(["arm-none-eabi-ld", "-Ttext=0xd0f90", "--defsym=stock_hook=0xd035d",
                        "--defsym=arena_allocate=0x17857", "-e", "hook", str(obj), "-o", str(elf)], check=True)
        subprocess.run(["arm-none-eabi-objcopy", "-O", "binary", str(elf), str(binary)], check=True)
        allocator = embedded_bytes("sram_allocator")
        assert len(allocator) == 112 and binary.read_bytes() == allocator
        assert allocator[84:92] == bytes.fromhex("f00f0d0060052000") and allocator[96:] == bytes(16)
        disassembly = subprocess.check_output(["arm-none-eabi-objdump", "-d", str(elf)], text=True)
        assert re.search(r"bl\s+17856", disassembly) and re.search(r"b.w\s+d035c", disassembly)
        assert "IPSR" in disassembly and "PRIMASK" in disassembly and "cpsid" in disassembly
        subprocess.run(["arm-none-eabi-as", "-mcpu=cortex-m3", "-mthumb", "--defsym", "SRAM_EXEC_PAYLOAD=1",
                        str(ROOT / "hardware/runtime_hook.S"), "-o", str(obj)], check=True)
        payload = embedded_bytes("sram_payload")
        assert len(payload) == 52 and payload[28:32] == bytes.fromhex("5d030d00") and payload[32:] == bytes(20)
        for address in (0x209bac, 0x217e78):
            subprocess.run(["arm-none-eabi-ld", f"-Ttext={address:#x}", "-e", "hook", str(obj), "-o", str(elf)], check=True)
            subprocess.run(["arm-none-eabi-objcopy", "-O", "binary", str(elf), str(binary)], check=True)
            assert binary.read_bytes() == payload, "SRAM payload must be position-independent"
        branch = temporary / "branch.S"
        branch.write_text(".syntax unified\n.thumb\n.global hook\n.thumb_func\nhook: b.w destination\n")
        subprocess.run(["arm-none-eabi-as", "-mcpu=cortex-m3", "-mthumb", str(branch), "-o", str(obj)], check=True)
        subprocess.run(["arm-none-eabi-ld", "-Ttext=0xd060c", "--defsym=destination=0xd0f91",
                        "-e", "hook", str(obj), "-o", str(elf)], check=True)
        subprocess.run(["arm-none-eabi-objcopy", "-O", "binary", str(elf), str(binary)], check=True)
        assert binary.read_bytes() == embedded_bytes("state7_branch")

    rom = (ROOT / "work/cyw43439-btrom-clean.bin").read_bytes()
    assert rom[0x6CFBC:0x6CFCA].hex() == "d6f800c03b464a4669462046e047", "unexpected command dispatch ABI"
    assert int.from_bytes(rom[0x6D2A0:0x6D2A4], "little") == 0x203A34
    table_initializer = re.search(r"uint32_t phase_stock_table\[\] = \{(.*?)\};", source, re.S).group(1)
    table = b"".join(int(value.strip(), 0).to_bytes(4, "little") for value in table_initializer.split(",") if value.strip())
    assert len(table) == 44 and table == rom[0x8af40:0x8af6c]
    assert int.from_bytes(rom[0x8e52c:0x8e530], "little") == 0x8af40
    assert 0x200718 + (0x8e52c - 0x8bc8c) == 0x202fb8
    assert rom[0x4917e:0x49184].hex() == "10b5044661b1", "phase callback ABI changed"
    patch = (ROOT / "work/sdk0065_extract/segment_000d0200_000d0f90.bin").read_bytes()
    expected = embedded_bytes("stock_prefix")
    assert patch[0x15C:0x15C + len(expected)] == expected
    assert patch[0x40C:0x410] == embedded_bytes("state7_prefix")
    assert embedded_bytes("state7_wrapper")[22:26] == embedded_bytes("state7_prefix")
    print("runtime probe: all wrappers, state7 branch, phase table and clean-ROM ABI checks pass")
    if arguments.prepare:
        prepare(arguments.phase, arguments.timing, arguments.sram)
    if arguments.capture:
        check_capture(arguments.capture, arguments.phase, arguments.timing, arguments.sram)


if __name__ == "__main__":
    main()
