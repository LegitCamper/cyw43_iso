#!/usr/bin/env python3
"""Check static RX evidence and an optional bounded marker snapshot; no hardware."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import runpy
import struct
import subprocess
import tempfile


def check_static():
    rom = (Path(__file__).resolve().parents[1] / "work/cyw43439-btrom-clean.bin").read_bytes()
    assert hashlib.sha256(rom).hexdigest() == "d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038"
    assert struct.unpack_from("<11I", rom, 0x8B634) == (
        0x712F3, 0x7131F, 0x71321, 0x71345, 0x71547, 0x713E3,
        0x7143F, 0x714CF, 0, 0x713FD, 0x7154F,
    )
    for address, value in {
        0x7158C: 0x209788, 0x71594: 0x2097B8, 0x715D0: 0x318BAC,
        0x480E8: 0x370A00, 0x4664C: 0x370A00, 0x46650: 0x370A10,
        0x46658: 0x370A0C, 0x70958: 0x318B98, 0x7095C: 0x370A00,
        0x70960: 0x318BAC,
    }.items():
        assert struct.unpack_from("<I", rom, address)[0] == value, hex(address)
    for address, encoded in {
        0x52424: "e9784900c171",
        0x712F2: "70b50446a5f7cefd054610f0010f03d0206881692046884715f0040f08bf70bd2068c1692046bde870400847",
        0x7143E: "70b504466ff001010020ecf7b5fa05466ff001010020ecf759fb95f8200084f82c000520a0815a48006810f0040f0ed0e1694a89521c4a8104f056f9032094f7d7fd206a78b12046bde87040abe7e16910f0020f14bfca898a8902f1010214bfca818a81e8e7",
        0x713FC: "10b50446006a40b1e07b012805d1032094f70ffe2046fff7e7ffe0698079002818bf10bd5b494868401e48600028c8bf10bd4ff446104269426157480089486010bd",
        0x714A4: "4ff446167569e07b01280cd1706920f0800070610220a5f72efef06945f0800540f00070f061756170bd",
        0x47FCA: "fcf7b2fa020008bfbde8f08194f8d80010f0020f18bf121f4149b068001d3df0e8ff",
        0x464D8: "5c4f14225c493ff06dfd",
        0x7080E: "534bd3f800c0c1f800c09b888b8090f86600c0f3801010707047",
        0x70830: "4b480068c0f3c00184f87d10c0f38001454884f87c100068a4f86600",
        0x85FBC: "2de9f003202a06dbb1e8f813a0e8f813a2f12002f6e7102a03db78c978c0a2f11002002a05d051f8043b40f8043b121ff9dcbde8f0037047",
    }.items():
        expected = bytes.fromhex(encoded)
        assert rom[address:address + len(expected)] == expected, hex(address)


def counter_result(before, status):
    after = list(before)
    index = 0 if status & 4 else 2 if status & 2 else 1
    after[index] = (after[index] + 1) & 0xFFFF
    return after

def csa2_channel(counter, channel_id=0xcf58):
    counter &= 0xffff
    value = counter ^ channel_id
    for _ in range(3):
        value = (int(f"{value & 255:08b}"[::-1], 2)
                 | (int(f"{value >> 8:08b}"[::-1], 2) << 8))
        value = (17 * value + channel_id) & 65535
    return (value ^ channel_id) % 37

def rf_selector(channel):
    return 0x80 | (2 * channel + (2 if channel <= 10 else 4))


def check_capture(record, expected_header, expected_payload):
    assert type(record) is dict
    assert type(record.get("version")) is int and record["version"] == 1
    assert record.get("complete") is True
    for key, value in (("event", 0x209788), ("parameters", 0x2097B8), ("mode", 1)):
        assert type(record.get(key)) is int and record[key] == value, key
    for key, maximum in (("ipsr", 0x1FF), ("status", 0xFFFFFFFF), ("header", 0xFFFFFFFF)):
        assert type(record.get(key)) is int and 0 <= record[key] <= maximum, key
    for key in ("before", "after"):
        values = record.get(key)
        assert type(values) is list and len(values) == 3, key
        assert all(type(value) is int and 0 <= value <= 0xFFFF for value in values), key
    assert record["after"] == counter_result(record["before"], record["status"]), "counter mismatch"
    assert record["status"] & 4, "positive marker requires stock bit2 branch"
    assert type(expected_header) is int and 0 <= expected_header <= 0xFFFFFFFF
    assert record["header"] == expected_header, "header mismatch"
    assert type(record.get("rssi")) is int and -128 <= record["rssi"] <= 127
    assert type(record.get("canaries")) is list and record["canaries"] == [0x52585331] * 2
    assert all(type(value) is int for value in record["canaries"])
    assert type(record.get("payload")) is str
    payload = bytes.fromhex(record["payload"])
    # ponytail: bounded 26/31-byte fixtures; widen only with another measured packet length.
    assert len(payload) == 32 and len(expected_payload) in (26, 31)
    assert payload[:len(expected_payload)] == expected_payload, "payload mismatch"


def probe_record(text, silent=False, bis=False, iso=False):
    assert not iso or bis, "ISO capture requires BIS variant"
    lines = [line.strip() for line in text.splitlines()]
    ready = [(index, line) for index, line in enumerate(lines) if line.startswith("RX_PROBE ready ")]
    assert len(ready) == 1, "require one RX ready marker"
    channel = 1 if bis else 0
    match = re.fullmatch(rf"RX_PROBE ready channel={channel} window-ms=20000 allocation=([0-9a-fA-F]{{8}})", ready[0][1])
    assert match, "invalid RX ready marker"
    allocation = int(match[1], 16)
    assert 0x209BAC <= allocation <= (0x21DC00 if bis else 0x21DE00) and allocation % 4 == 0, "allocation outside probe bounds"
    positions = [ready[0][0]]
    markers = (
        ("RX_PROBE stopped=", "RX_PROBE stopped=1 event-cleared=1"),
        ("RX_PROBE snapshot-stable=", f"RX_PROBE snapshot-stable=1 complete={0 if silent else 1}"),
        ("RX_PROBE global-restored=", "RX_PROBE global-restored=1"),
        ("RUNTIME_PROBE RX_PROBE-restored ", "RUNTIME_PROBE RX_PROBE-restored complete=1 stock-version=1"),
        ("RX_PROBE storage-retained-until-reset=", "RX_PROBE storage-retained-until-reset=1"),
        ("RX_PROBE lifecycle=", "RX_PROBE lifecycle=pass"),
    )
    for prefix, expected in markers:
        found = [(index, line) for index, line in enumerate(lines) if line.startswith(prefix)]
        assert len(found) == 1 and found[0][1] == expected, f"missing/failed/duplicate marker: {expected}"
        positions.append(found[0][0])
    if bis:
        configuration = [(index, line) for index, line in enumerate(lines) if line.startswith("RX_PROBE bis ")]
        assert len(configuration) == 1 and configuration[0][1] == (
            "RX_PROBE bis mhz=2404 logical-channel=0 aa=718ebed6 crc-init=123401 whitening=1"
        ), "missing/invalid BIS configuration"
        assert configuration[0][0] < ready[0][0], "BIS configuration after ready"
        phase = [(index, line) for index, line in enumerate(lines) if line.startswith("RX_PROBE bis-phase ")]
        assert len(phase) == 1, "require one BIS phase readback"
        match = re.fullmatch(r"RX_PROBE bis-phase count=([0-9]+) control=01302340 aa=718ebed6 "
                             r"crc=00123401 whitening=00000000 stable=1", phase[0][1])
        assert match and 0 < int(match[1]) <= 0xFFFFFFFF, "invalid BIS phase readback"
        positions.insert(3, phase[0][0])
    if iso:
        setup_positions = []
        for prefix, expected, before_ready in (
            ("RUNTIME_PROBE RX_ISO-installed ", "RUNTIME_PROBE RX_ISO-installed complete=1 stock-version=1", True),
            ("RX_ISO poll-assisted ", "RX_ISO poll-assisted interval-ms=250", True),
            ("RX_ISO task ", f"RX_ISO task delivered={0 if silent else 1} result=0 count={0 if silent else 1} errors=0 stable=1", False),
            ("RX_ISO transport-restored=", "RX_ISO transport-restored=1", False),
        ):
            found = [(index, line) for index, line in enumerate(lines) if line.startswith(prefix)]
            assert len(found) == 1 and found[0][1] == expected, f"missing/invalid ISO marker: {expected}"
            if before_ready:
                assert found[0][0] < ready[0][0], "ISO installation/poll marker after ready"
                setup_positions.append(found[0][0])
            else:
                positions.insert(4 if prefix == "RX_ISO task " else 5, found[0][0])
        assert setup_positions == sorted(setup_positions), "ISO setup markers out of order"
        wakes = [(index, line) for index, line in enumerate(lines) if line.startswith("RUNTIME_PROBE RX_ISO-wake ")]
        assert wakes and all(line == "RUNTIME_PROBE RX_ISO-wake complete=1 stock-version=1"
                             and positions[0] < index < positions[1] for index, line in wakes), "missing/failed ISO task wake"
    records = [(index, line.removeprefix("RX_PROBE record=")) for index, line in enumerate(lines)
               if line.startswith("RX_PROBE record=")]
    assert len(records) == 2, "require exactly two RX records"
    assert all(re.fullmatch(r"[0-9a-fA-F]{176}", item) for _, item in records), "invalid RX record encoding"
    raw = bytes.fromhex(records[0][1])
    assert bytes.fromhex(records[1][1]) == raw, "inconsistent record"
    positions[2:2] = [index for index, _ in records]
    assert positions == sorted(positions), "RX lifecycle markers out of order"
    canaries = [struct.unpack_from("<I", raw, offset)[0] for offset in (0, 84)]
    assert canaries == [0x52585331] * 2, "canary mismatch"
    state, version, ipsr, event, params, status, header = struct.unpack_from("<7I", raw, 4)
    assert version == 1, "record version"
    if iso:
        packets = [(index, line) for index, line in enumerate(lines) if line.startswith("RX_ISO packet=")]
        assert len(packets) == (0 if silent else 1), "ISO packet count mismatch"
        if not silent:
            match = re.fullmatch(r"RX_ISO packet=([0-9a-fA-F]{70}) length=34 count=1", packets[0][1])
            assert match, "invalid ISO packet encoding"
            expected = bytes.fromhex("0542201e0000001a00") + raw[52:78]
            assert bytes.fromhex(match[1]) == expected, "ISO header/sequence/payload mismatch"
            assert positions[0] < packets[0][0] < positions[1], "ISO packet outside receive window"
            assert header == 0x1A001A00, "ISO snapshot header mismatch"
    if silent:
        assert state == 0 and raw[12:84] == bytes(72), "silent control captured bit2 candidate or changed record"
        return None
    assert state == 2, "RX record not complete"
    rssi, mode = struct.unpack_from("<iI", raw, 44)
    return dict(version=version, complete=True, event=event, parameters=params,
                mode=mode, ipsr=ipsr, status=status, header=header,
                before=list(struct.unpack_from("<3H", raw, 32)),
                after=list(struct.unpack_from("<3H", raw, 38)), rssi=rssi,
                payload=raw[52:84].hex(), canaries=canaries)


def check_bad_crc(text, peer_text):
    probe_record(text, silent=True, bis=True, iso=True)
    lines = [line.strip() for line in text.splitlines()]
    counters = [(index, line) for index, line in enumerate(lines) if line.startswith("RX_ISO active-counters ")]
    assert len(counters) == 1, "require one active counter record"
    match = re.fullmatch(r"RX_ISO active-counters bit2=0 neither=([0-9]+) bit1=([0-9]+)", counters[0][1])
    assert match and int(match[1]) <= 65535 and 0 < int(match[2]) <= 65535, "no bounded bit1-only reception evidence"
    ready = next(index for index, line in enumerate(lines) if line.startswith("RX_PROBE ready "))
    stopped = lines.index("RX_PROBE stopped=1 event-cleared=1")
    assert ready < counters[0][0] < stopped, "counter record outside receive window"
    peer = runpy.run_path(str(Path(__file__).resolve().parents[1] / "hardware/nrf54-dtm-rx/check-marker-tx.py"))
    peer["check"](peer_text, 0, bis=True, length=26, bad_crc=True)


def self_test():
    marker = bytes(range(31))
    record = dict(version=1, complete=True, event=0x209788, parameters=0x2097B8,
                  mode=1, ipsr=16, status=4, header=0x1F00, before=[65535, 3, 7],
                  after=[0, 3, 7], rssi=-50, payload=(marker + b"\xff").hex(),
                  canaries=[0x52585331] * 2)
    check_capture(record, 0x1F00, marker)
    check_capture(record | {"header": 0x1A001A00}, 0x1A001A00, marker[:26])
    for length in (0, 25, 27, 32):
        try:
            check_capture(record, 0x1F00, bytes(range(length)))
        except AssertionError:
            continue
        raise AssertionError("accepted unsupported marker length")
    raw = bytearray(88)
    struct.pack_into("<8I", raw, 0, 0x52585331, 2, 1, 16, 0x209788, 0x2097B8, 4, 0x1F00)
    struct.pack_into("<6HiI", raw, 32, 65535, 3, 7, 0, 3, 7, -50, 1)
    raw[52:84] = marker + b"\xff"
    struct.pack_into("<I", raw, 84, 0x52585331)
    def capture(data, quiet=False):
        return ("RX_PROBE ready channel=0 window-ms=20000 allocation=00217e78\n"
                "RX_PROBE stopped=1 event-cleared=1\n"
                + ("RX_PROBE record=" + data.hex() + "\n") * 2
                + f"RX_PROBE snapshot-stable=1 complete={0 if quiet else 1}\n"
                "RX_PROBE global-restored=1\n"
                "RUNTIME_PROBE RX_PROBE-restored complete=1 stock-version=1\n"
                "RX_PROBE storage-retained-until-reset=1\nRX_PROBE lifecycle=pass\n")
    positive = capture(raw)
    assert probe_record(positive) == record
    bis_phase = "RX_PROBE bis-phase count=1 control=01302340 aa=718ebed6 crc=00123401 whitening=00000000 stable=1\n"
    bis_positive = ("RX_PROBE bis mhz=2404 logical-channel=0 aa=718ebed6 crc-init=123401 whitening=1\n"
                    + positive.replace("ready channel=0", "ready channel=1").replace(
                        "RX_PROBE global-restored=1", bis_phase + "RX_PROBE global-restored=1"))
    assert probe_record(bis_positive, bis=True) == record
    iso_raw = bytearray(raw)
    struct.pack_into("<I", iso_raw, 28, 0x1A001A00)
    iso_packet = "RX_ISO packet=" + (bytes.fromhex("0542201e0000001a00") + marker[:26]).hex() + " length=34 count=1\n"
    iso_task = "RX_ISO task delivered=1 result=0 count=1 errors=0 stable=1\n"
    iso_restore = "RX_ISO transport-restored=1\n"
    iso_positive = ("RUNTIME_PROBE RX_ISO-installed complete=1 stock-version=1\n"
                    "RX_ISO poll-assisted interval-ms=250\n" + bis_positive.replace(raw.hex(), iso_raw.hex())
                    .replace("RX_PROBE stopped=", "RUNTIME_PROBE RX_ISO-wake complete=1 stock-version=1\n" + iso_packet + "RX_PROBE stopped=")
                    .replace("RX_PROBE global-restored=", iso_task + iso_restore + "RX_PROBE global-restored="))
    assert probe_record(iso_positive, bis=True, iso=True) == record | {"header": 0x1A001A00}
    silent = struct.pack("<3I", 0x52585331, 0, 1) + bytes(72) + struct.pack("<I", 0x52585331)
    assert probe_record(capture(silent, quiet=True), silent=True) is None
    iso_silent = (iso_positive.replace(iso_raw.hex(), silent.hex()).replace(iso_packet, "")
                  .replace("snapshot-stable=1 complete=1", "snapshot-stable=1 complete=0")
                  .replace(iso_task, "RX_ISO task delivered=0 result=0 count=0 errors=0 stable=1\n"))
    assert probe_record(iso_silent, silent=True, bis=True, iso=True) is None
    counters = "RX_ISO active-counters bit2=0 neither=0 bit1=500\n"
    crc_negative = iso_silent.replace("RX_PROBE stopped=", counters + "RX_PROBE stopped=")
    peer_negative = ("DTM_TX ready mhz=2404 mode=BLE1M aa=718ebed6 crc_init=123400 whitening=true "
                     "header=1a00 payload=[" + ", ".join(f"{value:02x}" for value in range(26))
                     + "]\nDTM_TX done sent=500 disabled=true\n")
    check_bad_crc(crc_negative, peer_negative)
    for bad, peer in ((iso_silent, peer_negative), (crc_negative + counters, peer_negative),
                      (crc_negative.replace("bit1=500", "bit1=0"), peer_negative),
                      (crc_negative.replace("bit2=0", "bit2=1"), peer_negative),
                      (crc_negative.replace("bit1=500", "bit1=65536"), peer_negative),
                      (counters + iso_silent, peer_negative),
                      (crc_negative, peer_negative.replace("123400", "123401")),
                      (crc_negative, peer_negative.replace("sent=500", "sent=0")),
                      (crc_negative + iso_packet, peer_negative)):
        try:
            check_bad_crc(bad, peer)
        except AssertionError:
            continue
        raise AssertionError("accepted invalid CRC rejection evidence")
    invalid_iso = [bis_positive, iso_positive + iso_packet, iso_positive.replace(iso_packet, ""),
                   iso_positive.replace("0542201e0000001a00", "0542201e0001001a00"),
                   iso_positive.replace("delivered=1", "delivered=0"),
                   iso_positive.replace("transport-restored=1", "transport-restored=0"),
                   iso_positive.replace(iso_restore, "") + iso_restore,
                   iso_positive.replace("RX_ISO-wake complete=1", "RX_ISO-wake complete=0")]
    invalid_iso += [iso_positive.replace(line + "\n", "", 1) for line in iso_positive.splitlines()
                    if "RX_ISO" in line]
    for text in invalid_iso:
        try:
            probe_record(text, bis=True, iso=True)
        except AssertionError:
            continue
        raise AssertionError("accepted invalid ISO delivery evidence")
    invalid = [(capture(raw[:-1]), False), (capture(raw, quiet=True), True),
               (capture(silent), False), (positive, True),
               (positive.replace(raw.hex(), silent.hex(), 1), False),
               (positive + "RX_PROBE record=" + raw.hex(), False),
               (positive + "RX_PROBE lifecycle=unproven\n", False),
               (positive.replace("RX_PROBE global-restored=1", "RX_PROBE global-restored=0"), False),
               (positive.replace("allocation=00217e78", "allocation=ffffffff"), False),
               (positive.replace("RX_PROBE stopped=1 event-cleared=1\n", "") + "RX_PROBE stopped=1 event-cleared=1\n", False)]
    invalid += [(positive.replace(line + "\n", "", 1), False) for line in positive.splitlines()]
    for text, quiet in invalid:
        try:
            probe_record(text, silent=quiet)
        except AssertionError:
            continue
        raise AssertionError("accepted invalid probe record")
    for text in (positive, bis_positive.replace("allocation=00217e78", "allocation=0021de00"),
                 bis_positive.replace("count=1", "count=0"), bis_positive.replace("stable=1\nRX_PROBE global", "stable=0\nRX_PROBE global"),
                 bis_positive.replace("control=01302340", "control=01002340"),
                 bis_positive.replace(bis_phase, ""), bis_positive + bis_phase,
                 bis_phase + bis_positive.replace(bis_phase, "")):
        try:
            probe_record(text, bis=True)
        except AssertionError:
            continue
        raise AssertionError("accepted invalid BIS channel/allocation")
    for status in range(8):
        expected = [65535] * 3
        expected[0 if status & 4 else 2 if status & 2 else 1] = 0
        assert counter_result([65535] * 3, status) == expected
    for key, bad in (("status", 2), ("after", [1, 3, 7]), ("header", 0x1E00),
                     ("payload", bytes(32).hex()), ("payload", marker.hex()),
                     ("complete", False), ("event", 0), ("mode", True),
                     ("canaries", [0, 0]), ("before", [-1, 3, 7])):
        try:
            check_capture(record | {key: bad}, 0x1F00, marker)
        except AssertionError:
            continue
        raise AssertionError(f"accepted bad {key}")


def task_module():
    path = Path(__file__).with_name("check-iso-rx-task.py")
    spec = importlib.util.spec_from_file_location("iso_rx_task", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def build(bis=False, iso=False, stream=False, hop=False, timer_hop=False):
    assert not iso or bis, "ISO task delivery requires BIS variant"
    assert not stream or iso, "stream requires ISO task delivery"
    assert not hop or stream, "hopping requires streaming ISO delivery"
    assert not timer_hop or stream, "timer hopping requires streaming ISO delivery"
    assert not (hop and timer_hop), "packet and timer hopping are separate images"
    root = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix="controller-rx-") as directory:
        obj, elf, binary = (Path(directory) / name for name in ("rx.o", "rx.elf", "rx.bin"))
        subprocess.run(["arm-none-eabi-as", "-mcpu=cortex-m3", "-mthumb"]
                       + (["--defsym", "BIS_PACKET=1"] if bis else [])
                       + (["--defsym", "RX_STREAM=1"] if stream else [])
                       + (["--defsym", "RX_HOP=1"] if hop else [])
                       + (["--defsym", "RX_TIMER_HOP=1"] if timer_hop else []) + [
                        str(root / "hardware/controller_rx.S"), "-o", str(obj)], check=True)
        blobs = []
        for base in (0x209BAC, 0x217E78):
            subprocess.run(["arm-none-eabi-ld", f"-Ttext={base:#x}", "-e", "hook", str(obj), "-o", str(elf)], check=True)
            subprocess.run(["arm-none-eabi-objcopy", "-O", "binary", str(elf), str(binary)], check=True)
            blobs.append(binary.read_bytes())
            symbols = subprocess.check_output(["arm-none-eabi-nm", "-n", str(elf)], text=True)
            values = {name: int(address, 16) for address, _, name in (line.split() for line in symbols.splitlines())}
            for name, offset in (("callbacks", 256), ("guard_before", 316), ("trace", 320), ("guard_after", 400)):
                assert values[name] == base + offset, name
            if bis:
                assert values["bis_phase"] == base + 416
                assert values["bis_trace"] == base + 768
            if stream:
                assert values["stream_forward"] == base + 912
            if hop:
                for name, offset in (("hop_advance", 1024), ("hop_initial", 1050), ("hop_trace", 1072),
                                     ("configure_channel", 1152), ("bis_channel", 1280)):
                    assert values[name] == base + offset, name
            if timer_hop:
                for name, offset in (("hop_advance", 1024), ("hop_timer_initial", 1050), ("hop_trace", 1096),
                                     ("timer_countdown", 1120), ("configure_channel", 1280), ("bis_channel", 1408)):
                    assert values[name] == base + offset, name
        size = 2048 if hop or timer_hop else 1024 if bis else 512
        assert blobs[0] == blobs[1] and len(blobs[0]) == size
        blob = blobs[0]
        rom = (root / "work/cyw43439-btrom-clean.bin").read_bytes()
        assert blob[256:300] == rom[0x8B634:0x8B660]
        assert struct.unpack_from("<III", blob, 316) == (0x52585331, 0, 1)
        assert blob[328:400] == bytes(72)
        assert struct.unpack_from("<I", blob, 400)[0] == 0x52585331
        if bis:
            assert blob[404:416] == bytes(12)
            assert blob[768:912 if stream else 1024] == bytes(144 if stream else 256)
            if hop:
                assert blob[1084:1152] == bytes(68)
                assert blob[1220:1280] == bytes(60)
                assert blob[1400:] == bytes(648)
            if timer_hop:
                assert blob[1120:1122] != bytes(2)
                assert blob[1232:1280] == bytes(48)
                assert blob[1348:1408] == bytes(60)
                assert blob[1528:] == bytes(520)
        else:
            assert blob[404:] == bytes(108)
        if iso:
            overlay = task_module().build(stream=stream)
            assert blob[576:576 + len(overlay)] == bytes(len(overlay)), "task overlay overlaps BIS image"
            blob = blob[:576] + overlay + blob[576 + len(overlay):]
        return blob


def emulate(blob, stream=False, hop=False, timer_hop=False):
    import unicorn as un
    from unicorn import arm_const as arm

    rom = (Path(__file__).resolve().parents[1] / "work/cyw43439-btrom-clean.bin").read_bytes()
    registers = [getattr(arm, f"UC_ARM_REG_R{i}") for i in range(13)]
    base, event, params, rssip, stop, stack = 0x217E78, 0x209788, 0x2097B8, 0x206000, 0xF0000, 0x21D000
    trace = base + 320
    hop_trace = base + 1072
    marker = bytes(range(32))

    def run(status, primask=0, wrapped=True, bad=None, state=0, pending=0, ipsr=0,
            wrong_header=False, context=True, event_counter=0):
        cpu = un.Uc(un.UC_ARCH_ARM, un.UC_MODE_THUMB | un.UC_MODE_MCLASS)
        cpu.mem_map(0, 0x100000)
        cpu.mem_write(0, rom)
        cpu.mem_map(0x200000, 0x20000)
        cpu.mem_write(base, blob)
        cpu.mem_map(0x318000, 0x1000)
        cpu.mem_map(0x370000, 0x1000)
        def put(at, value):
            cpu.mem_write(at, struct.pack("<I", value))
        put(event + 28, params if bad != "parameters" else 0x208000)
        put(event + 32, pending)
        cpu.mem_write(event + 15, b"\x01")
        cpu.mem_write(params + 6, bytes([0 if bad == "mode" else 1]))
        cpu.mem_write(params + 10, struct.pack("<3H", 65535, 65535, 65535))
        put(0x318BAC, status)
        header = 0x1A001A00 if stream and not wrong_header else 0x1F00
        put(0x318B98, header)
        put(0x201F1C, 0x206200 if context else 0)
        put(0x318014, 0x123456F8)
        put(0x31801C, 0x23456789)
        channel_hop = hop or timer_hop
        current_channel = csa2_channel(event_counter) if channel_hop else 0
        identity = {0x318B6C: 0x01302340, 0x318B70: 0x718EBED6,
                    0x318BA4: 0x123401, 0x318BF4: current_channel,
                    0x3186A4: rf_selector(current_channel)}
        for address, value in identity.items():
            put(address, value)
        cpu.mem_write(0x370A00, marker)
        put(trace, state)
        if hop:
            cpu.mem_write(hop_trace, struct.pack("<3I", event_counter, current_channel, 1))
        elif timer_hop:
            cpu.mem_write(base + 1096, struct.pack("<6I", event_counter, current_channel, 1, 32, 0, 0))
        incoming = [0x11000000 + i * 0x101 for i in range(13)]
        incoming[0] = event if bad != "event" else 0x209000
        for register, value in zip(registers, incoming):
            cpu.reg_write(register, value)
        cpu.reg_write(arm.UC_ARM_REG_SP, stack)
        cpu.reg_write(arm.UC_ARM_REG_LR, stop | 1)
        cpu.reg_write(arm.UC_ARM_REG_APSR, 0xA8000000)
        cpu.reg_write(arm.UC_ARM_REG_PRIMASK, primask)
        cpu.reg_write(arm.UC_ARM_REG_IPSR, ipsr)
        calls, reads, writes = [], [], []
        skipped = bad is not None

        def code(machine, address, _size, _user):
            if address == 0x7143E:
                calls.append("stock")
                assert machine.reg_read(arm.UC_ARM_REG_SP) % 8 == 0
                for index in (0, 1, 2, 3, 12):
                    assert machine.reg_read(registers[index]) == incoming[index]
                assert machine.reg_read(arm.UC_ARM_REG_APSR) & 0xF8000000 == 0xA8000000
                assert machine.reg_read(arm.UC_ARM_REG_PRIMASK) == primask
                if not skipped:
                    return
            elif address in (0x5D9B6, 0x5DB0A, 0x75726, 0x602E, 0x1711A, 0x1C97E, 0x9060):
                calls.append(address)
                assert machine.reg_read(arm.UC_ARM_REG_SP) % 8 == 0
                if address == 0x9060:
                    assert calls[0] == "stock"
                    assert [machine.reg_read(registers[i]) for i in range(3)] == [0x2062B4, 1, 0]
                    assert struct.unpack("<I", machine.mem_read(trace, 4))[0] == 2
                    assert machine.reg_read(arm.UC_ARM_REG_PRIMASK) == primask
                if address == 0x75726:
                    assert machine.reg_read(registers[0]) == status
                if address == 0x5DB0A:
                    machine.mem_write(rssip + 32, bytes([206]))
            else:
                return
            for index in (0, 1, 2, 3, 12):
                machine.reg_write(registers[index], 0xCA000000 + index)
            if address == 0x5D9B6:
                machine.reg_write(registers[0], rssip)
            machine.reg_write(arm.UC_ARM_REG_APSR, 0x58000000)
            machine.reg_write(arm.UC_ARM_REG_PC, machine.reg_read(arm.UC_ARM_REG_LR))

        def memory(_machine, access, address, size, value, _user):
            if access == un.UC_MEM_READ:
                reads.append((address, size))
            else:
                writes.append((address, size, value))
                if address == trace and value == 2:
                    assert cpu.mem_read(trace + 48, 32) == marker
                    assert list(struct.unpack("<3H", cpu.mem_read(trace + 34, 6))) == counter_result([65535] * 3, status)

        cpu.hook_add(un.UC_HOOK_CODE, code)
        cpu.hook_add(un.UC_HOOK_MEM_READ | un.UC_HOOK_MEM_WRITE, memory)
        cpu.emu_start((base if wrapped else 0x7143E) | 1, stop, count=10000)
        assert cpu.reg_read(arm.UC_ARM_REG_PC) == stop
        assert cpu.reg_read(arm.UC_ARM_REG_SP) == stack
        assert cpu.reg_read(arm.UC_ARM_REG_PRIMASK) == primask
        assert calls.count("stock") == 1
        for index in range(4, 12):
            assert cpu.reg_read(registers[index]) == incoming[index], index
        assert cpu.mem_read(base + 316, 4) == struct.pack("<I", 0x52585331)
        assert cpu.mem_read(base + 400, 4) == struct.pack("<I", 0x52585331)
        for address, value in identity.items():
            expected = value
            if hop and wrapped and not skipped and address in (0x318BF4, 0x3186A4):
                next_channel = csa2_channel((event_counter + 1) & 0xffff)
                expected = next_channel if address == 0x318BF4 else rf_selector(next_channel)
            actual = bytes(cpu.mem_read(address, 4))
            assert actual == struct.pack("<I", expected), (hex(address), event_counter, actual.hex(), expected)
        if hop:
            advanced = int(wrapped and not skipped)
            next_counter = (event_counter + advanced) & 0xffff
            actual_hop = bytes(cpu.mem_read(hop_trace, 12))
            expected_hop = struct.pack("<3I", next_counter, csa2_channel(next_counter), 1 + advanced)
            assert actual_hop == expected_hop, (actual_hop.hex(), expected_hop.hex(), status, bad)
        elif timer_hop:
            actual_hop = bytes(cpu.mem_read(base + 1096, 24))
            expected_hop = struct.pack("<6I", event_counter, current_channel, 1, 32, 0, 0)
            assert actual_hop == expected_hop, (actual_hop.hex(), expected_hop.hex(), status, bad)
        result = [cpu.reg_read(register) for register in registers]
        result.append(cpu.reg_read(arm.UC_ARM_REG_APSR) & 0xF8000000)
        window_reads = [(address, size) for address, size in reads if 0x370000 <= address < 0x371000]
        status_reads = [(address, size) for address, size in reads if address == 0x318BAC]
        assert status_reads == [(0x318BAC, 4)] * (0 if skipped else 2 if wrapped else 1)
        captured = wrapped and not skipped and state == 0 and status & 4 and not wrong_header
        if captured:
            assert window_reads == [(0x370A00 + i * 4, 4) for i in range(8)]
            record = bytes(cpu.mem_read(trace, 80))
            assert struct.unpack_from("<2I", record) == (2, 1)
            assert struct.unpack_from("<I", record, 8)[0] == ipsr
            assert struct.unpack_from("<4I", record, 12) == (event, params, status, header)
            assert struct.unpack_from("<3H", record, 28) == (65535,) * 3
            assert list(struct.unpack_from("<3H", record, 34)) == counter_result([65535] * 3, status)
            assert struct.unpack_from("<iI", record, 40) == (-50, 1)
            assert record[48:] == marker
        else:
            assert not window_reads
            expected = bytearray(blob[320:400])
            struct.pack_into("<I", expected, 0, state)
            assert cpu.mem_read(trace, 80) == expected
        assert calls.count(0x9060) == int(bool(stream and wrapped and not skipped and context and (captured or state == 2)))
        assert all(not (0x318000 <= address < 0x319000) for address, _, _ in writes if skipped)
        return (result, bytes(cpu.mem_read(event, 48)), bytes(cpu.mem_read(params, 24)),
                [(address, size, value) for address, size, value in writes if 0x318000 <= address < 0x319000])

    def equivalent(arguments):
        wrapped = run(**arguments)
        stock = run(wrapped=False, **arguments)
        if hop and arguments.get("bad") is None:
            assert wrapped[:3] == stock[:3]
            stock_mmio = stock[3]
            wrapped_mmio = wrapped[3]
            assert wrapped_mmio[:len(stock_mmio)] == stock_mmio
            channel = csa2_channel((arguments.get("event_counter", 0) + 1) & 0xffff)
            assert wrapped_mmio[len(stock_mmio):] == [
                (0x318BF4, 4, channel), (0x3186A4, 4, rf_selector(channel))]
        else:
            assert wrapped == stock

    for status in range(8):
        for mask in (0, 1):
            equivalent({"status": status, "primask": mask})
    for bad in ("event", "parameters", "mode"):
        equivalent({"status": 4, "bad": bad})
    for state in (1, 2):
        equivalent({"status": 4, "state": state})
    equivalent({"status": 4, "pending": 1})
    equivalent({"status": 4, "ipsr": 16})
    if hop:
        for counter in (0, 1, 0xfffe, 0xffff):
            equivalent({"status": 4, "event_counter": counter})
    if stream:
        for state in (0, 1, 2):
            for status in range(8):
                equivalent({"status": status, "state": state, "wrong_header": True})
        equivalent({"status": 4, "context": False})
    print("RX assembly passes: first-bit2 eligibility, stock branches, guards, wrap, repeated-call forwarding, ABI and MMIO equivalence"
          + (", post-stock CSA#2 retune and event-counter wrap." if hop
             else ", RX callbacks do not advance timer-driven hopping." if timer_hop else "."))


def emulate_phase(blob, hop=False, timer_hop=False):
    import unicorn as un
    from unicorn import arm_const as arm

    rom = (Path(__file__).resolve().parents[1] / "work/cyw43439-btrom-clean.bin").read_bytes()
    registers = [getattr(arm, f"UC_ARM_REG_R{i}") for i in range(13)]
    base, event, params, stop, stack = 0x217E78, 0x209788, 0x2097B8, 0xF0000, 0x21D000
    identity = {0x318B6C: 0x01302340, 0x318B70: 0x718EBED6,
                0x318BA4: 0x123401, 0x318BF4: 0}

    def run(bad=None, wrapped=True, count=0, primask=0):
        cpu = un.Uc(un.UC_ARCH_ARM, un.UC_MODE_THUMB | un.UC_MODE_MCLASS)
        cpu.mem_map(0, 0x100000)
        cpu.mem_write(0, rom)
        cpu.mem_map(0x200000, 0x20000)
        cpu.mem_write(base, blob)
        cpu.mem_map(0x318000, 0x1000)
        def put(address, value):
            cpu.mem_write(address, struct.pack("<I", value))
        put(event + 28, params if bad != "parameters" else 0x208000)
        cpu.mem_write(params + 6, bytes([0 if bad == "mode" else 1, 1 if bad == "channel" else 2]))
        cpu.mem_write(event + 14, b"\x05\x01")
        put(0x203158, 0x555555)
        put(0x20315C, 0x71764129)
        put(base + 768, count)
        incoming = [0x11000000 + i * 0x101 for i in range(13)]
        incoming[0] = event if bad != "event" else 0x209000
        incoming[1] = 1 if bad == "phase" else 0
        for register, value in zip(registers, incoming):
            cpu.reg_write(register, value)
        cpu.reg_write(arm.UC_ARM_REG_SP, stack)
        cpu.reg_write(arm.UC_ARM_REG_LR, stop | 1)
        cpu.reg_write(arm.UC_ARM_REG_APSR, 0xA8000000)
        cpu.reg_write(arm.UC_ARM_REG_PRIMASK, primask)
        calls, writes = [], []

        def code(machine, address, _size, _user):
            if address == 0x71546:
                calls.append("stock")
                assert machine.reg_read(arm.UC_ARM_REG_SP) % 8 == 0
                for index in (0, 1, 2, 3, 12):
                    assert machine.reg_read(registers[index]) == incoming[index]
                assert machine.reg_read(arm.UC_ARM_REG_APSR) & 0xF8000000 == 0xA8000000
                if bad is None:
                    return
            elif address in (0x602E, 0x1A71E, 0x1B1E6):
                calls.append(address)
                assert machine.reg_read(arm.UC_ARM_REG_SP) % 8 == 0
                if address == 0x1B1E6:
                    assert machine.reg_read(registers[0]) == 5
                    assert machine.reg_read(registers[1]) == 1
            else:
                return
            for index in (0, 1, 2, 3, 12):
                machine.reg_write(registers[index], 0xCA000000 + index)
            machine.reg_write(arm.UC_ARM_REG_APSR, 0x58000000)
            machine.reg_write(arm.UC_ARM_REG_PC, machine.reg_read(arm.UC_ARM_REG_LR))

        def memory(machine, _access, address, size, value, _user):
            writes.append((address, size, value))
            if address == base + 768:
                assert list(struct.unpack("<4I", machine.mem_read(base + 772, 16))) == list(identity.values())
                assert value == (count + 1) & 0xFFFFFFFF

        cpu.hook_add(un.UC_HOOK_CODE, code)
        cpu.hook_add(un.UC_HOOK_MEM_WRITE, memory)
        cpu.emu_start((base + 416 if wrapped else 0x71546) | 1, stop, count=10000)
        assert cpu.reg_read(arm.UC_ARM_REG_PC) == stop
        assert cpu.reg_read(arm.UC_ARM_REG_SP) == stack
        assert cpu.reg_read(arm.UC_ARM_REG_PRIMASK) == primask
        assert calls.count("stock") == 1
        for index in range(4, 12):
            assert cpu.reg_read(registers[index]) == incoming[index], index
        assert cpu.mem_read(base + 316, 88) == blob[316:404]
        mmio = [(address, size, value) for address, size, value in writes if 0x318000 <= address < 0x319000]
        if wrapped and bad is None:
            channel = csa2_channel(0) if hop or timer_hop else 0
            assert bytes(cpu.mem_read(0x3186A4, 4)) == struct.pack("<I", rf_selector(channel))
            for address, value in identity.items():
                expected = channel if (hop or timer_hop) and address == 0x318BF4 else value
                assert bytes(cpu.mem_read(address, 4)) == struct.pack("<I", expected), hex(address)
            expected_mmio = [(0x318B70, 4, 0x718EBED6), (0x318BA4, 4, 0x123401),
                             (0x318B6C, 4, 0x01302340), (0x318BF4, 4, 0)]
            if hop or timer_hop:
                expected_mmio += [(0x318BF4, 4, channel), (0x3186A4, 4, rf_selector(channel))]
                trace_offset = 1096 if timer_hop else 1072
                assert bytes(cpu.mem_read(base + trace_offset, 12)) == struct.pack("<3I", 0, channel, 1)
            assert mmio[-len(expected_mmio):] == expected_mmio
            assert struct.unpack("<I", cpu.mem_read(base + 768, 4))[0] == (count + 1) & 0xFFFFFFFF
            if timer_hop:
                assert struct.unpack("<I", cpu.mem_read(base + 256 + 36, 4))[0] == (base + 1120) | 1
            mmio = mmio[:-len(expected_mmio)]
        else:
            assert bytes(cpu.mem_read(base + 768, 20)) == struct.pack("<I", count) + bytes(16)
        result = [cpu.reg_read(register) for register in registers]
        return result, cpu.reg_read(arm.UC_ARM_REG_APSR) & 0xF8000000, mmio, calls

    for bad in (None, "phase", "event", "parameters", "mode", "channel"):
        for primask in (0, 1):
            assert run(bad, primask=primask) == run(bad, wrapped=False, primask=primask)
    for count in (1, 0xFFFFFFFF):
        assert run(count=count) == run(wrapped=False, count=count)
    print("BIS phase assembly passes: ROM setup, post-stock identity, guards, repeated phase, count wrap, ABI and ordered writes"
          + ("; event zero starts on its CSA#2 channel."
             if hop or timer_hop else "; RX rearm preserves identity with external helpers stubbed."))


def emulate_timer_hop(blob):
    import unicorn as un
    from unicorn import arm_const as arm

    rom = (Path(__file__).resolve().parents[1] / "work/cyw43439-btrom-clean.bin").read_bytes()
    registers = [getattr(arm, f"UC_ARM_REG_R{i}") for i in range(13)]
    base, event, params, stop, stack = 0x217E78, 0x209788, 0x2097B8, 0xF0000, 0x21D000
    trace, callback = base + 1096, base + 1120
    cpu = un.Uc(un.UC_ARCH_ARM, un.UC_MODE_THUMB | un.UC_MODE_MCLASS)
    cpu.mem_map(0, 0x100000)
    cpu.mem_write(0, rom)
    cpu.mem_map(0x200000, 0x20000)
    cpu.mem_write(base, blob)
    cpu.mem_map(0x318000, 0x1000)
    cpu.mem_write(event + 28, struct.pack("<I", params))
    cpu.mem_write(params + 6, b"\x01")
    first_channel = csa2_channel(0)
    cpu.mem_write(trace, struct.pack("<6I", 0, first_channel, 1, 32, 0, 0))
    cpu.mem_write(0x318BF4, struct.pack("<I", first_channel))
    cpu.mem_write(0x3186A4, struct.pack("<I", rf_selector(first_channel)))
    stock_calls, writes = [], []

    def code(machine, address, _size, _user):
        if address != 0x713FC:
            return
        stock_calls.append(machine.reg_read(arm.UC_ARM_REG_R0))
        machine.reg_write(arm.UC_ARM_REG_PC, machine.reg_read(arm.UC_ARM_REG_LR))

    def memory(_machine, access, address, size, value, _user):
        if access == un.UC_MEM_WRITE and 0x318000 <= address < 0x319000:
            writes.append((address, size, value))

    cpu.hook_add(un.UC_HOOK_CODE, code)
    cpu.hook_add(un.UC_HOOK_MEM_WRITE, memory)

    def tick(argument=event, mode=1):
        cpu.mem_write(params + 6, bytes([mode]))
        incoming = [0x11000000 + i * 0x101 for i in range(13)]
        incoming[0] = argument
        for register, value in zip(registers, incoming):
            cpu.reg_write(register, value)
        cpu.reg_write(arm.UC_ARM_REG_SP, stack)
        cpu.reg_write(arm.UC_ARM_REG_LR, stop | 1)
        before_writes = len(writes)
        cpu.emu_start(callback | 1, stop, count=10000)
        assert cpu.reg_read(arm.UC_ARM_REG_PC) == stop
        assert cpu.reg_read(arm.UC_ARM_REG_SP) == stack
        for index in range(4, 9):
            assert cpu.reg_read(registers[index]) == incoming[index], index
        return writes[before_writes:]

    initial = bytes(cpu.mem_read(trace, 24))
    assert tick(argument=0x209000) == [] and bytes(cpu.mem_read(trace, 24)) == initial
    assert tick(mode=0) == [] and bytes(cpu.mem_read(trace, 24)) == initial
    for _ in range(31):
        assert tick() == []
    assert struct.unpack("<6I", cpu.mem_read(trace, 24)) == (0, first_channel, 1, 1, 31, 0)
    channel = csa2_channel(1)
    assert tick() == [(0x318BF4, 4, channel), (0x3186A4, 4, rf_selector(channel))]
    assert struct.unpack("<6I", cpu.mem_read(trace, 24)) == (1, channel, 2, 32, 32, 1)
    for index in range(64):
        updates = tick()
        if index in (31, 63):
            counter = 2 + index // 32
            expected_channel = csa2_channel(counter)
            assert updates == [(0x318BF4, 4, expected_channel),
                               (0x3186A4, 4, rf_selector(expected_channel))]
        else:
            assert updates == []
    channel = csa2_channel(3)
    assert struct.unpack("<6I", cpu.mem_read(trace, 24)) == (3, channel, 4, 32, 96, 3)
    cpu.mem_write(trace, struct.pack("<6I", 0xFFFF, csa2_channel(0xFFFF), 9, 1, 100, 8))
    assert tick() == [(0x318BF4, 4, csa2_channel(0)), (0x3186A4, 4, rf_selector(csa2_channel(0)))]
    assert struct.unpack("<6I", cpu.mem_read(trace, 24)) == (0, csa2_channel(0), 10, 32, 101, 9)
    assert stock_calls == [0x209000, event] + [event] * 97
    print("Timer-hop assembly passes: stock countdown chaining, 32-tick cadence, RX-independent missed-packet recovery, guards and 16-bit wrap.")


def export_blob(directory, blob, stream=False, hop=False, timer_hop=False):
    root = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix="rx-allocator-") as temporary:
        obj, elf, binary = [Path(temporary) / name for name in ("allocator.o", "allocator.elf", "allocator.bin")]
        reservation = 2048 if hop or timer_hop else 1024
        subprocess.run(["arm-none-eabi-as", "-mcpu=cortex-m3", "-mthumb", "--defsym", "SRAM_EXEC_ALLOC=1",
                        "--defsym", f"SRAM_RESERVATION_SIZE={reservation}", str(root / "hardware/runtime_hook.S"),
                        "-o", str(obj)], check=True)
        subprocess.run(["arm-none-eabi-ld", "-Ttext=0xd0f90", "--defsym=stock_hook=0xd035d",
                        "--defsym=arena_allocate=0x17857", "-e", "hook", str(obj), "-o", str(elf)], check=True)
        subprocess.run(["arm-none-eabi-objcopy", "-O", "binary", str(elf), str(binary)], check=True)
        allocator = binary.read_bytes()
    source = (root / "hardware/picow_runtime_probe.c").read_text()
    raw = re.search(r"uint8_t sram_allocator\[\] = \{(.*?)\};", source, re.S)[1]
    expected = bytearray(int(value.strip(), 0) for value in raw.split(",") if value.strip())
    expected[56:58] = bytes.fromhex("0060" if hop or timer_hop else "8060")
    assert allocator == expected and len(allocator) == 112 and len(blob) == reservation
    manifest = {
        "abi_version": 1, "controller": "CYW43439", "firmware": "0065",
        "diagnostic": "one_shot_fixed_channel_rx", "runtime_install_required": True,
        "rom_sha256": "d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038",
        "bootstrap_address": 0xD0F90, "bootstrap_requires_zero_reservation": True,
        "boot_firmware_sha256": "b3c0c99774135c32beea47af847838077a1f0b7a6be661a68ddcd3641362ad7d",
        "allocation_bytes": reservation, "handle": 0x42, "sdu_octets": 26,
        "transport": {"direction": "controller_to_host", "hci_packet_type": 5,
                      "hci_iso_data_load_octets": 30, "packet_boundary_flag": 2,
                      "timestamp_flag": 0, "packet_status_flag": 0,
                      "diagnostic_sequence": 0, "maximum_deliveries": 1,
                      "host_poll_interval_ms": 250},
        "radio": {"phy": "LE_1M", "hci_test_channel": 1, "frequency_mhz": 2404,
                  "logical_channel": 0, "access_address": 0x718EBED6,
                  "crc_init": 0x123401, "whitening": True, "expected_raw_header": 0x1A001A00},
        "pointer_installation": {"transport_context_pointer_address": 0x201F1C,
                                 "transport_callback_context_offset": 0x100,
                                 "expected_transport_callback": 0x44105,
                                 "radio_callback_table_pointer_address": 0x203A80,
                                 "expected_radio_callback_table": 0x8B634,
                                 "phase_table_slot": 4, "rx_table_slot": 6},
        "resident_offsets": {"rx_complete": 0, "callback_table": 256,
                             "guard_before": 316, "snapshot": 320, "payload": 368,
                             "guard_after": 400, "phase": 416, "transport": 576,
                             "task_state": 736, "phase_trace": 768, "emitter": 800},
        "limitations": ["fixed_channel_no_BIG_synchronization_or_hopping", "one_snapshot_not_streaming",
                        "first_bit2_packet_claims_snapshot_even_if_unsupported",
                        "poll_assisted_task_wake", "no_standard_BIG_creation_CIS_or_encryption",
                        "exclusive_stock_DTM_event_no_radio_coexistence", "reset_only_resident_reclamation"],
        "files": {},
    }
    if stream:
        manifest["diagnostic"] = "single_mailbox_fixed_channel_rx"
        manifest["transport"].update(diagnostic_sequence="accepted_sdu_counter_low16", maximum_deliveries=None,
                                     host_poll_interval_ms=0, task_wake="radio_callback_event_or_0x9061")
        manifest["resident_offsets"].update(rejected_snapshots=744, isr_forward=912)
        manifest["limitations"] = ["fixed_channel_no_BIG_synchronization_or_hopping", "one_lossy_snapshot_mailbox",
                                   "accepted_SDU_sequence_not_radio_sequence", "no_RF_loss_accounting",
                                   "ring_full_retry_requires_later_radio_callback", "no_standard_BIG_creation_CIS_or_encryption",
                                   "exclusive_stock_DTM_event_no_radio_coexistence", "reset_only_resident_reclamation"]
    if hop:
        manifest["diagnostic"] = "open_loop_csa2_hopping_rx"
        manifest["radio"].update(hci_test_channel=None, frequency_mhz=None, logical_channel=None,
                                 channel_selection_algorithm=2,
                                 channel_id=0xCF58, event_counter_initial=0,
                                 channel_map="all_37_data_channels")
        manifest["resident_offsets"].update(hop_advance=1024, hop_initial=1050, hop_trace=1072,
                                            channel_config=1152, csa2=1280)
        manifest["limitations"] = ["open_loop_counter_without_BIGInfo_anchor", "one_lossy_snapshot_mailbox",
                                   "counter_advances_once_per_stock_RX_callback", "no_RF_loss_accounting",
                                   "no_standard_BIG_creation_CIS_or_encryption",
                                   "exclusive_stock_DTM_event_no_radio_coexistence", "reset_only_resident_reclamation"]
    if timer_hop:
        manifest["diagnostic"] = "dtm_countdown_csa2_hopping_rx"
        manifest["radio"].update(hci_test_channel=None, frequency_mhz=None, logical_channel=None,
                                 channel_selection_algorithm=2, channel_id=0xCF58,
                                 event_counter_initial=0, channel_map="all_37_data_channels",
                                 nominal_interval_us=20000, countdown_ticks_per_event=32)
        manifest["pointer_installation"].update(countdown_table_slot=9,
                                                 countdown_slot_installed_by_phase_wrapper=True)
        manifest["resident_offsets"].update(hop_advance=1024, hop_initial=1050, hop_trace=1096,
                                            countdown=1120, channel_config=1280, csa2=1408)
        manifest["limitations"] = ["open_loop_counter_without_BIGInfo_anchor", "one_lossy_snapshot_mailbox",
                                   "625us_DTM_scheduler_tick_is_static_not_measured_for_this_image",
                                   "retune_boundary_not_live_validated", "no_RF_loss_accounting",
                                   "no_standard_BIG_creation_CIS_or_encryption",
                                   "exclusive_stock_DTM_event_no_radio_coexistence", "reset_only_resident_reclamation"]
    directory.mkdir(parents=True, exist_ok=True)
    for name, data in (("native.bin", blob), ("allocator.bin", allocator)):
        (directory / name).write_bytes(data)
        manifest["files"][name] = {"size": len(data), "sha256": hashlib.sha256(data).hexdigest()}
    (directory / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    capture = parser.add_mutually_exclusive_group()
    capture.add_argument("--capture", type=Path)
    capture.add_argument("--probe-capture", type=Path)
    negative = parser.add_mutually_exclusive_group()
    negative.add_argument("--silent", action="store_true")
    negative.add_argument("--bad-crc", action="store_true", help="require observed CRC-init mismatch rejection and peer log")
    parser.add_argument("--peer-capture", type=Path)
    parser.add_argument("--expected-header", type=lambda value: int(value, 0))
    parser.add_argument("--expected-payload", type=bytes.fromhex)
    parser.add_argument("--emulate", action="store_true")
    parser.add_argument("--bis", action="store_true", help="fixed-channel BIS-format RX variant")
    parser.add_argument("--iso", action="store_true", help="one-shot task ISO delivery; requires --bis")
    parser.add_argument("--stream", action="store_true", help="rearm one mailbox and wake from RX; requires --bis --iso")
    parser.add_argument("--hop", action="store_true", help="open-loop CSA#2 retune after each stock RX callback; requires --stream")
    parser.add_argument("--timer-hop", action="store_true", help="CSA#2 retune every 32 stock DTM scheduler ticks; requires --stream")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--output-header", type=Path)
    parser.add_argument("--output-blob", type=Path, help="export --bis --iso image, allocator and ABI manifest")
    args = parser.parse_args()
    if args.iso and not args.bis:
        parser.error("--iso requires --bis")
    if args.stream and not args.iso:
        parser.error("--stream requires --bis --iso")
    if args.hop and not args.stream:
        parser.error("--hop requires --stream")
    if args.timer_hop and not args.stream:
        parser.error("--timer-hop requires --stream")
    if args.hop and args.timer_hop:
        parser.error("--hop and --timer-hop select separate images")
    if args.stream and (args.capture or args.probe_capture):
        parser.error("stream capture evidence uses the normal-runner stream checker")
    if args.output_blob and not args.iso:
        parser.error("--output-blob requires --bis --iso")
    if args.bad_crc and not (args.iso and args.probe_capture and args.peer_capture):
        parser.error("--bad-crc requires --bis --iso --probe-capture and --peer-capture")
    if args.peer_capture and not args.bad_crc:
        parser.error("--peer-capture is only used with --bad-crc")
    if args.iso and args.capture:
        parser.error("--iso requires --probe-capture for delivery evidence")
    if args.iso and args.expected_payload is not None and len(args.expected_payload) != 26:
        parser.error("--iso positive capture requires a 26-byte expected payload")
    if args.silent and not args.probe_capture:
        parser.error("--silent requires --probe-capture")
    if (args.capture or args.probe_capture) and not (args.silent or args.bad_crc) and (args.expected_header is None or args.expected_payload is None):
        parser.error("positive capture requires --expected-header and --expected-payload")
    check_static()
    self_test()
    blob = build(bis=args.bis, iso=args.iso, stream=args.stream, hop=args.hop, timer_hop=args.timer_hop)
    if args.output:
        args.output.write_bytes(blob)
    if args.output_header:
        rows = ["    " + ", ".join(f"0x{value:02x}" for value in blob[offset:offset + 16]) + ","
                for offset in range(0, len(blob), 16)]
        args.output_header.write_text("/* Generated by analysis/check-controller-rx.py. */\n"
                                      f"static const uint8_t controller_rx[{len(blob)}] = {{\n" + "\n".join(rows) + "\n};\n")
    if args.emulate:
        emulate(blob, stream=args.stream, hop=args.hop, timer_hop=args.timer_hop)
        if args.bis:
            emulate_phase(blob, hop=args.hop, timer_hop=args.timer_hop)
        if args.timer_hop:
            emulate_timer_hop(blob)
        if args.iso:
            task = task_module()
            task.check(blob[576:576 + len(task.build(stream=args.stream))], stream=args.stream)
            if args.stream:
                task.check_interleaving(blob)
    if args.output_blob:
        export_blob(args.output_blob, blob, stream=args.stream, hop=args.hop, timer_hop=args.timer_hop)
    if args.bad_crc:
        check_bad_crc(args.probe_capture.read_text(), args.peer_capture.read_text())
        print("CRC-init mismatch control passes: bit1 activity, no bit2 snapshot or HCI ISO delivery; general CRC/status semantics unproven.")
        return
    if args.capture or args.probe_capture:
        record = (json.loads(args.capture.read_text()) if args.capture
                  else probe_record(args.probe_capture.read_text(), silent=args.silent, bis=args.bis, iso=args.iso))
        if args.silent:
            print("RX silent control record unchanged; no bit2 candidate captured during observed window.")
            return
        check_capture(record, args.expected_header, args.expected_payload)
        print("RX snapshot and one poll-assisted ISO task delivery pass; continuous RX and BIS synchronization unproven."
              if args.iso else "RX marker snapshot passes; continuous RX and BIS synchronization unproven.")
    else:
        print("RX static evidence and checker self-test pass; no live RX result claimed.")


if __name__ == "__main__":
    main()
