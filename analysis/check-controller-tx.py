#!/usr/bin/env python3
"""Check resident custom test-radio payload and optional live TX capture."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import runpy
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def crc24(packet, initial=0x555555):
    """Core Vol 6 Part B 3.1.1, register orientation used by Nordic RXCRC."""
    crc = initial
    for octet in packet:
        for bit in range(8):
            feedback = ((crc >> 23) ^ (octet >> bit)) & 1
            crc = (crc << 1) & 0xffffff
            if feedback:
                crc ^= 0x65b
    return crc


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--capture", type=Path)
    parser.add_argument("--peer-capture", type=Path, help="require a CRC-valid complete custom payload from receiver")
    parser.add_argument("--bis", action="store_true", help="check BIS identity and whitening variant")
    parser.add_argument("--audio", action="store_true", help="check four rotating LC3 frames and timed order")
    parser.add_argument("--hop", action="store_true", help="check full-map CSA #2 audio hopping")
    parser.add_argument("--discovery", action="store_true", help="check mixed advertising and BIS image")
    parser.add_argument("--scan", action="store_true", help="check three-primary-channel discovery image")
    parser.add_argument("--native", action="store_true", help="check host-fed HCI ISO discovery image")
    parser.add_argument("--continuous", action="store_true", help="check sustained host ISO with continuous discovery")
    parser.add_argument("--output-blob", type=Path, help="export checked native image, allocator and LC3 fixture")
    parser.add_argument("--require-discovery", action="store_true", help="require scan-derived SDC acquisition with --native --peer-capture")
    args = parser.parse_args()
    args.native |= args.continuous
    if args.output_blob and not args.native:
        parser.error("--output-blob needs --native or --continuous")
    if args.require_discovery and not (args.native and args.peer_capture):
        parser.error("--require-discovery needs --native and --peer-capture")
    args.scan |= args.native
    args.discovery |= args.scan
    args.hop |= args.discovery
    args.audio |= args.hop
    args.bis |= args.audio
    reference = runpy.run_path(str(ROOT / "analysis/check-bis-channel.py"))["reference"]
    if args.hop:
        subprocess.run(["python3", str(ROOT / "analysis/check-bis-channel.py"), "--output", str(ROOT / "work/bis-channel")], check=True)
    rom = (ROOT / "work/cyw43439-btrom-clean.bin").read_bytes()
    assert hashlib.sha256(rom).hexdigest() == "d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038"
    table = [int.from_bytes(rom[i:i+4], "little") for i in range(0x8b634, 0x8b660, 4)]
    assert table == [0x712f3, 0x7131f, 0x71321, 0x71345, 0x71547, 0x713e3, 0x7143f, 0x714cf, 0, 0x713fd, 0x7154f]
    source = (ROOT / "hardware/picow_runtime_probe.c").read_text()
    variant = "controller_continuous_iso" if args.continuous else "controller_native_iso" if args.native else "controller_discovery_scan" if args.scan else "controller_discovery" if args.discovery else "controller_hop" if args.hop else "controller_audio" if args.audio else "controller_bis" if args.bis else "controller_tx"
    raw = re.search(rf"uint8_t {variant}\[\] = \{{(.*?)\}};", source, re.S)[1]
    payload = bytes(int(value.strip(), 0) for value in raw.split(",") if value.strip())
    reservation = 4096 if args.native else 2048 if args.discovery else 1024 if args.hop else 512
    assert len(payload) == reservation and payload[224:320] == bytes(96)
    frames = [payload[320 + 28*i:346 + 28*i] for i in range(4)] if args.audio else []
    if args.native:
        raw = re.search(r"uint8_t native_audio\[\] = \{(.*?)\};", source, re.S)[1]
        native_frames = bytes(int(value.strip(), 0) for value in raw.split(",") if value.strip())
        assert len(native_frames) == 520
    if args.audio:
        assert len(set(frames)) == 4 and payload[432:448] == bytes(16)
        for i in range(4):
            assert payload[346 + 28*i:348 + 28*i] == bytes(2)
    else:
        assert payload[320:360] == bytes(range(40))
    with tempfile.TemporaryDirectory() as directory:
        obj, elf, binary = (Path(directory) / name for name in ("tx.o", "tx.elf", "tx.bin"))
        defines = ["--defsym", "BIS_PACKET=1"] if args.bis else []
        if args.hop:
            defines += ["--defsym", "HOP_PACKET=1"]
        if args.discovery:
            defines += ["--defsym", "DISCOVERY_PACKET=1"]
            if args.scan:
                defines += ["--defsym", "SCAN_PACKET=1"]
            if args.native:
                defines += ["--defsym", "NATIVE_ISO_PACKET=1"]
            if args.continuous:
                defines += ["--defsym", "CONTINUOUS_ISO_PACKET=1"]
            subprocess.run(["cargo", "run", "--quiet", "--example", "broadcast_fixture", "--", directory]
                           + (["--continuous"] if args.continuous else []), cwd=ROOT, check=True)
            for name, offset in (("primary", 1024), ("auxiliary", 1088), ("periodic", 1152)):
                packet = (Path(directory) / f"discovery-{name}.bin").read_bytes()
                assert payload[offset:offset+len(packet)] == packet, "packet bank differs from Rust builder"
            compiler_flags = ["-Os", "-DMULTICHANNEL_DISCOVERY"] if args.scan else ["-O2"]
            if args.native:
                compiler_flags += ["-DNATIVE_ISO_QUEUE", "-DREPEATED_DISCOVERY"]
            if args.continuous:
                compiler_flags += ["-DCONTINUOUS_DISCOVERY"]
            subprocess.run(["arm-none-eabi-gcc", "-std=c11", *compiler_flags, "-mcpu=cortex-m3", "-mthumb",
                            "-ffreestanding", "-fPIC", "-fno-builtin", "-Wall", "-Wextra", "-Werror",
                            "-c", str(ROOT / "hardware/discovery_packet.c"), "-o", str(obj)], check=True)
            assert not subprocess.check_output(["arm-none-eabi-nm", "-u", str(obj)], text=True).strip()
            helper_offset = 3328 if args.native else 1280
            for base in (0x209bac, 0x217e78):
                address = base + helper_offset
                subprocess.run(["arm-none-eabi-ld", f"-Ttext={address:#x}", "-e", "discovery_packet", str(obj), "-o", str(elf)], check=True)
                subprocess.run(["arm-none-eabi-objcopy", "-O", "binary", "--only-section=.text", str(elf), str(binary)], check=True)
                helper = binary.read_bytes()
                assert len(helper) <= (768 if args.native else 352)
                assert payload[helper_offset:helper_offset+len(helper)] == helper
            if args.native:
                transport_directory = Path(directory) / "transport"
                credit_file = Path(directory) / "credits.bin"
                subprocess.run(["python3", str(ROOT / "analysis/check-transport-ingress.py"),
                                "--output", str(transport_directory)], check=True)
                subprocess.run(["python3", str(ROOT / "analysis/check-iso-credits.py"),
                                "--output", str(credit_file)], check=True)
                transport = (transport_directory / "transport.bin").read_bytes()
                credits = credit_file.read_bytes()
                assert len(transport) <= 1024 and len(credits) == 148
                assert payload[2048:2048+len(transport)] == transport
                assert payload[3072:3072+len(credits)] == credits
                assert payload[1792:2048] == bytes(256)
                assert payload[2056:2064] == bytes(4) + (0x42).to_bytes(4, "little")
                assert payload[2088:2096] == bytes(8)
        assembly = "controller_audio.S" if args.audio else "controller_tx.S"
        subprocess.run(["arm-none-eabi-as", "-mcpu=cortex-m3", "-mthumb", *defines, str(ROOT / "hardware" / assembly), "-o", str(obj)], cwd=ROOT, check=True)
        if args.audio:
            generated = Path(directory) / "audio.lc3"
            subprocess.run(["cargo", "run", "--quiet", "--features", "lc3", "--example", "radio_audio", "--", "generate", str(generated)], cwd=ROOT, check=True)
            assert generated.read_bytes() == b"".join(frames), "embedded frames differ from codec fixture"
            if args.native:
                subprocess.run(["cargo", "run", "--quiet", "--features", "lc3", "--example", "radio_audio", "--", "generate-native", str(generated)], cwd=ROOT, check=True)
                assert generated.read_bytes() == native_frames, "host SDUs differ from 20-frame codec fixture"
        for address in (0x209bac, 0x217e78):
            subprocess.run(["arm-none-eabi-ld", f"-Ttext={address:#x}", "-e", "hook", str(obj), "-o", str(elf)], check=True)
            subprocess.run(["arm-none-eabi-objcopy", "-O", "binary", str(elf), str(binary)], check=True)
            assert binary.read_bytes() == payload
            if args.discovery:
                disassembly = subprocess.check_output(["arm-none-eabi-objdump", "-d", str(elf)], text=True)
                bases = re.findall(r"([0-9a-f]+):[^\n]*\bsubw\s+r0, pc, #(\d+)", disassembly)
                assert len(bases) == 2, "expected two allocation-base calculations"
                for instruction, immediate in bases:
                    actual = ((int(instruction, 16) + 4) & ~3) - int(immediate)
                    assert actual == address, "ADR passed a Thumb-tagged data pointer"
        symbols = subprocess.check_output(["arm-none-eabi-nm", "-n", str(elf)], text=True)
        values = {name: int(address, 16) for address, _, name in (line.split() for line in symbols.splitlines())}
        for name, offset in (("tx_complete", 128 if args.audio else 160), ("callbacks", 224), ("trace", 272), ("payload", 320)):
            assert values[name] - values["hook"] == offset
        if args.hop:
            assert values["configure_channel"] - values["hook"] == 512
            assert values["bis_channel"] - values["hook"] == 640
            raw_allocator = re.search(r"uint8_t sram_allocator\[\] = \{(.*?)\};", source, re.S)[1]
            allocator = bytearray(int(v.strip(), 0) for v in raw_allocator.split(",") if v.strip())
            immediate = bytes.fromhex("8050") if args.native else bytes([0 if args.discovery else 0x80, 0x60])
            for offset, byte in enumerate(immediate, 56):
                assert f"image[{offset}] = 0x{byte:02x};" in source
            allocator[56:58] = immediate
            subprocess.run(["arm-none-eabi-as", "-mcpu=cortex-m3", "-mthumb", "--defsym", "SRAM_EXEC_ALLOC=1",
                            "--defsym", f"SRAM_RESERVATION_SIZE={reservation}", str(ROOT / "hardware/runtime_hook.S"), "-o", str(obj)], check=True)
            subprocess.run(["arm-none-eabi-ld", "-Ttext=0xd0f90", "--defsym=stock_hook=0xd035d",
                            "--defsym=arena_allocate=0x17857", "-e", "hook", str(obj), "-o", str(elf)], check=True)
            subprocess.run(["arm-none-eabi-objcopy", "-O", "binary", str(elf), str(binary)], check=True)
            assert binary.read_bytes() == allocator, "allocator patch differs from assembly"
    if args.output_blob:
        args.output_blob.mkdir(parents=True, exist_ok=True)
        blobs = {"native.bin": payload, "allocator.bin": allocator, "audio.lc3": native_frames}
        manifest = {"abi_version": 1, "controller": "CYW43439", "firmware": "0065",
                    "rom_sha256": hashlib.sha256(rom).hexdigest(), "continuous_discovery": args.continuous,
                    "handle": 66, "sample_rate_hz": 8000, "frame_duration_us": 10000,
                    "sdu_octets": 26, "iso_credits": 4, "runtime_install_required": True,
                    "bootstrap_address": 0xd0f90,
                    "bootstrap_requires_zero_reservation": True,
                    "boot_firmware_sha256": "b3c0c99774135c32beea47af847838077a1f0b7a6be661a68ddcd3641362ad7d",
                    "transport": {"direction": "host_to_controller", "hci_packet_type": 5,
                                  "hci_iso_data_load_octets": 30, "packet_boundary_flag": 2,
                                  "timestamp_flag": 0, "packet_status_flag": 0,
                                  "unsupported_iso_behavior": "blocks_transport_ring_head",
                                  "credit_completion": "radio_completion_not_queue_acceptance"},
                    "radio": {"bis_count": 1, "iso_interval_us": 10000, "phy": "LE_1M",
                              "framed": False, "encrypted": False, "bn": 1, "nse": 1,
                              "irc": 1, "pto": 0, "channel_map_hex": "ffffffff1f"},
                    "limitations": ["exclusive_stock_DTM_event_no_radio_coexistence",
                                    "no_standard_BIG_creation_or_CIS",
                                    "fixed_handle_and_SDU_format", "reset_only_resident_reclamation"],
                    "pointer_installation": {"code_pointer_thumb_bit": 1,
                                             "allocation_base_u32_offset": 2056,
                                             "credit_emitter_pointer_u32_offset": 2088,
                                             "reported_completions_u32_offset": 2092,
                                             "transport_context_pointer_address": 0x201f1c,
                                             "transport_callback_context_offset": 0x100,
                                             "expected_transport_callback": 0x44105,
                                             "command_lookup_pointer_address": 0x203a34,
                                             "expected_command_lookup": 0xd035d,
                                             "radio_callback_table_pointer_address": 0x203a80,
                                             "expected_radio_callback_table": 0x8b634},
                    "resident_offsets": {"phase": 0, "tx_complete": 128, "callback_table": 224,
                                         "queue": 1792, "queue_slots": 1824,
                                         "transport": 2048, "transport_state": 2056,
                                         "credit_lookup": 3072, "credit_emitter": 3160,
                                         "radio_consumer": 3328}, "files": {}}
        for name, blob in blobs.items():
            (args.output_blob / name).write_bytes(blob)
            manifest["files"][name] = {"size": len(blob), "sha256": hashlib.sha256(blob).hexdigest()}
        (args.output_blob / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    if args.capture:
        text = args.capture.read_text()
        if args.native:
            for marker in ("NATIVE_ISO buffers iso-length=30 iso-count=4 acl-preserved=1",
                           "NATIVE_ISO refill-pause-ms=80 after-sent=8",
                           "NATIVE_ISO transport-restored=1", "NATIVE_ISO lookup-restored=1",
                           "RUNTIME_PROBE NATIVE_ISO-installed complete=1 stock-version=1",
                           "RUNTIME_PROBE NATIVE_ISO-drained complete=1 stock-version=1"):
                assert marker in text, marker
            final = re.search(r"NATIVE_ISO sent=(\d+) accepted=(\d+) consumed=(\d+) completed=(\d+) reported=(\d+) credits=(\d+) underruns=(\d+) inflight=(\d+) head=(\d+) tail=(\d+) blocked=(\d+) errors=(\d+)", text)
            assert final, "missing native queue accounting"
            values = list(map(int, final.groups()))
            target = 1000 if args.continuous else 20
            assert values[:6] == [target] * 6, "host, FIFO, RF and credit totals disagree"
            assert values[6] > 0 and values[7] == values[10] == values[11] == 0
            assert 0 <= values[8] == values[9] < 5
            total = 0
            for count, reported in re.findall(r"NATIVE_ISO credit=(\d+) total=(\d+)", text):
                assert 1 <= int(count) <= 4
                total += int(count)
                assert total == int(reported) <= target
            assert total == target, "completion events missing or duplicated"
        if args.hop:
            before = re.search(rf"SRAM_PROBE bounds initialized=1 cursor=([0-9a-f]+) end=([0-9a-f]+) size={reservation}", text)
            after = re.search(r"SRAM_PROBE allocation=([0-9a-f]+) cursor-after=([0-9a-f]+) end-after=([0-9a-f]+)", text)
            assert before and after
            assert int(before[1], 16) == int(after[1], 16)
            assert int(after[2], 16) == int(before[1], 16) + reservation
            assert before[2] == after[3]
        if args.bis:
            identity = re.search(r"TX_PROBE bis-aa=718ebed6 crc-init=00123401 control=([0-9a-f]+) whitening-channel=(\d+)", text)
            if args.discovery and identity is None:
                identity = re.search(r"TX_PROBE bis-aa=12345678 crc-init=00abcdef control=([0-9a-f]+) whitening-channel=(\d+)", text)
            if args.continuous and identity is None:
                identity = re.search(r"TX_PROBE bis-aa=8e89bed6 crc-init=00555555 control=([0-9a-f]+) whitening-channel=(\d+)", text)
            assert identity and int(identity[1], 16) & 0x300000 == 0x300000
            assert 0 <= int(identity[2]) <= (39 if args.continuous else 36 if args.hop else 0)
        for marker in ("SRAM_PROBE result=pass", "TX_PROBE command=201e complete=1 status=00",
                       "TX_PROBE command=201f complete=1 status=00", "TX_PROBE stopped=1 event-cleared=1",
                       "TX_PROBE global-restored=1", "TX_PROBE storage-retained-until-reset=1",
                       "RUNTIME_PROBE TX_PROBE-restored complete=1 stock-version=1"):
            assert marker in text, marker
        match = re.search(r"TX_PROBE status=(\d+) original=([0-9a-f]+) restored=([0-9a-f]+) tx-callbacks=(\d+) ipsr=(\d+) stock-count=(\d+) mode=([0-9a-f]+) headers=([0-9a-f]+) payload-prefix-match=(\d)", text)
        assert match and int(match[6]) > 0
        observed = bytes.fromhex(re.search(r"TX_PROBE payload=([0-9a-f]+)", text)[1])
        if "table-installed mode=control" in text:
            assert match[1] == match[4] == match[9] == "0"
            assert observed[:31] != bytes(range(31)) and "TX_PROBE result=control-pass" in text
        else:
            assert match[1] == "2" and match[2] == match[3] and int(match[4]) > 0 and int(match[5]) > 0
            assert match[9] == "1" and "TX_PROBE result=pass" in text
            if args.audio:
                if args.discovery:
                    assert re.search(r"TX_PROBE audio frames=4 octets=26 reload=(4|6|10)\b", text)
                    prefixes = [payload[offset+2:offset+6] for offset in (1024, 1088, 1152)]
                else:
                    assert "TX_PROBE audio frames=4 octets=26 reload=16" in text
                    prefixes = []
                if not args.native:
                    assert observed[:4] in [frame[:4] for frame in frames] + prefixes
            else:
                assert observed[:4] == bytes(range(4))
                snapshot = bytes.fromhex(re.search(r"TX_PROBE controller-snapshot=([0-9a-f]+)", text)[1])
                assert snapshot[:4] == bytes(range(4)) and "TX_PROBE payload-readback=first-word-only" in text
    if args.peer_capture:
        if args.native:
            with tempfile.TemporaryDirectory() as directory:
                fixture = Path(directory) / "native.lc3"
                fixture.write_bytes(native_frames)
                command = ["python3", str(ROOT / "analysis/check-sdc-capture.py"),
                           str(args.peer_capture), "--fixture", str(fixture),
                           "--stream-fixture" if args.continuous else "--native-fixture"]
                if args.require_discovery:
                    command.append("--require-discovery")
                subprocess.run(command, check=True)
            print("native HCI ISO image, accounting and requested SDC capture checks pass")
            return
        if args.discovery:
            subprocess.run(["python3", str(ROOT / "hardware/nrf54-dtm-rx/check-discovery.py"), str(args.peer_capture)], check=True)
            print("controller discovery image and requested capture checks pass")
            return
        text = args.peer_capture.read_text()
        identity = "aa=0x718ebed6 crc_init=0x123401 whitening=true channel=0" if args.bis else "aa=0x71764129 crc_init=0x555555 whitening=off"
        if args.hop:
            identity = identity[:-1] + str(reference(0, 0xcf58))
        assert identity in text
        records = re.findall(r"DTM_RX packet=(\d+) crc_ok=true rx_crc=0x([0-9a-f]+) raw=\[([^\]]+)\]", text)
        custom = bytes([0, 31]) + bytes(range(31))
        stock = bytes.fromhex("001fffc1fbe84c90728be7b3518963ab232302841872aa612f3b51a8e53749fbc9")
        counts = {bytes([0, 26]) + frame: 0 for frame in frames} if args.audio else {custom: 0, stock: 0}
        timestamps = {int(number): int(address) for number, address in re.findall(
            r"DTM_RX timing packet=(\d+) valid=true first=(?:true|false) address_us=(\d+)", text)}
        previous = None
        ordered = 0
        for number, checksum, record in records:
            packet = bytes(int(value.strip(), 16) for value in record.split(","))
            assert len(packet) >= 2 and packet[1] == len(packet) - 2
            assert crc24(packet, 0x123401 if args.bis else 0x555555) == int(checksum, 16), "software CRC disagrees with receiver"
            assert packet in counts, "unexpected CRC-valid DTM payload"
            assert not args.bis or args.audio or packet == custom
            counts[packet] += 1
            if args.audio and int(number) in timestamps:
                index, timestamp = frames.index(packet[2:]), timestamps[int(number)]
                if previous is not None:
                    delta = (timestamp - previous[1]) & 0xffffffff
                    steps = (delta + 5000) // 10000
                    assert steps > 0 and abs(delta - steps * 10000) <= 10
                    assert index == (previous[0] + steps) % 4, "LC3 frame order disagrees with RF timestamps"
                    ordered += 1
                previous = index, timestamp
        if args.audio:
            assert all(counts.values()), "not all four LC3 frames received"
            assert ordered >= 3, "too few timestamped LC3 transitions"
            print(f"LC3 frame counts={list(counts.values())}; timed order checks={ordered}; software CRC agrees")
            if args.hop:
                assert "hopping_desync" not in text
                hops = re.findall(r"DTM_RX hop packet=(\d+) counter=(\d+) channel=(\d+) next_counter=(\d+) next_channel=(\d+)", text)
                assert len(hops) >= 4, "too few hopping records"
                received = {int(n): bytes(int(v.strip(), 16) for v in raw.split(",")) for n, _, raw in records}
                for number, counter, channel, following, next_channel in hops:
                    number, counter, channel, following, next_channel = map(int, (number, counter, channel, following, next_channel))
                    assert following == (counter + 1) & 0xffff
                    assert channel == reference(counter, 0xcf58)
                    assert next_channel == reference(following, 0xcf58)
                    if number in received:
                        assert received[number][2:] == frames[counter % 4]
                print(f"CSA #2 channel/LC3 alignment checks={len(hops)}")
        else:
            assert counts[custom] > 0, "no CRC-valid complete custom packet"
            print(f"logged packets: custom={counts[custom]} stock={counts[stock]}; software CRC agrees")
    print("controller TX assembly, layout, callback table and requested capture checks pass")


if __name__ == "__main__":
    main()
