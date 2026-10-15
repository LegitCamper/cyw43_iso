#!/usr/bin/env python3
"""Validate stock SDC BIS reception against independent transmitted LC3 fixture."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile
import wave

ROOT = Path(__file__).resolve().parents[1]
PREFIX = "NRF54_BROADCAST_RX "
BASE = bytes.fromhex("1f165118409c00010106000000000a02010102020103041a0004030204000100")
ISO = re.compile(r"iso count=(\d+) seq=(\d+) status=(\d+) len=(\d+) valid=(true|false) data=\[([^]]*)\]")


def check(text, fixture, last_transmitted_seq=None, native=False, require_discovery=False, stream=False,
          allow_underrun=False):
    if native and stream:
        raise ValueError("native-once and cyclic-stream fixture modes are exclusive")
    if allow_underrun and not stream:
        raise ValueError("underrun allowance requires cyclic-stream fixture mode")
    frame_total = 20 if native or stream else 4
    if len(fixture) != frame_total * 26:
        raise ValueError(f"fixture must contain {frame_total} 26-byte SDUs")
    frames = [fixture[i:i + 26] for i in range(0, len(fixture), 26)]
    if len(set(frames)) != frame_total:
        raise ValueError("fixture SDUs must be distinguishable")
    periodic = biginfo = base_seen = big = ready = ended = False
    discovered = preconfigured = False
    packets, output, frame_counts = [], bytearray(), Counter()
    counts = Counter()
    last_count = last_seq = None
    matched_indices, recoveries = [], []
    previous_valid_seq = None
    sequence_position = previous_valid_position = None
    gap_since_valid = False
    lost_since_valid = 0
    inferred_padding = []
    for number, line in enumerate(text.splitlines(), 1):
        if PREFIX not in line:
            continue
        message = line.split(PREFIX, 1)[1]
        if message.startswith("preconfigured-identity-sync-armed"):
            preconfigured = True
        elif message.startswith("discovered "):
            encoded = message.split("data=[", 1)[1].split("]", 1)[0]
            if periodic or bytes.fromhex(encoded.replace(",", " ")) != bytes.fromhex("06165218563412"):
                raise ValueError(f"line {number}: unexpected scan-derived announcement")
            discovered = True
        elif message.startswith("periodic-established "):
            if periodic or "status: Success," not in message:
                raise ValueError(f"line {number}: require one successful periodic session")
            for field in ("adv_sid: 3,", "adv_addr_kind: AddrKind(1)",
                          "adv_addr: BdAddr([1, 2, 3, 4, 5, 198])", "adv_phy: Le1M"):
                if field not in message:
                    raise ValueError(f"line {number}: unexpected periodic identity/PHY")
            periodic = True
        elif message.startswith("periodic-data status=Complete data="):
            encoded = message.split("data=[", 1)[1].split("]", 1)[0]
            if bytes.fromhex(encoded.replace(",", " ")) == BASE:
                base_seen = True
        elif message.startswith("biginfo "):
            expected = ("num_bis: 1,", "nse: 1,", "iso_interval: 8,", "bn: 1,",
                        "pto: 0,", "irc: 1,", "max_pdu: 26,", "max_sdu: 26,",
                        "sdu_interval: ExtDuration([16, 39, 0])", "phy: Le1M,",
                        "is_framed: false,", "is_encrypted: false")
            biginfo = all(field in message for field in expected)
        elif message.startswith("big-established "):
            if big or not periodic or not base_seen or not biginfo or "status: Success," not in message:
                raise ValueError(f"line {number}: BIG success lacks matching periodic/BASE/BIGInfo")
            big = True
        elif message.startswith("iso-ready "):
            if not big:
                raise ValueError(f"line {number}: ISO ready before BIG success")
            ready = True
        elif message.startswith("big-lost "):
            ended = True
        elif message.startswith("iso-fragment "):
            raise ValueError(f"line {number}: fragmented ISO outside this fixture")
        elif message.startswith("iso "):
            match = ISO.fullmatch(message)
            if match is None or not ready or ended:
                raise ValueError(f"line {number}: malformed ISO or no active successful BIG")
            count, seq, status, length = map(int, match.groups()[:4])
            valid = match[5] == "true"
            data = bytes.fromhex(match[6].replace(",", " "))
            if not 0 <= seq <= 65535 or not 0 < count <= 0xFFFFFFFF or status not in (0, 1, 2):
                raise ValueError(f"line {number}: invalid count/sequence/status")
            if last_count is not None:
                count_delta = (count - last_count) & 0xFFFFFFFF
                seq_delta = (seq - last_seq) & 0xFFFF
                if not 0 < count_delta < 0x80000000 or not 0 < seq_delta < 0x8000:
                    raise ValueError(f"line {number}: duplicate/reordered ISO")
                counts["missing_log_records"] += count_delta - 1
                counts["sequence_gaps"] += seq_delta - 1
                gap_since_valid |= seq_delta > 1 and previous_valid_seq is not None
                sequence_position += seq_delta
            else:
                sequence_position = seq
            last_count, last_seq = count, seq
            if valid != (status == 0 and length == 26 and len(data) == 26):
                raise ValueError(f"line {number}: validity contradicts SDU fields")
            if status in (0, 1) and (length != 26 or len(data) != length):
                raise ValueError(f"line {number}: unexpected fixture SDU length")
            if status == 2 and (length or data):
                raise ValueError(f"line {number}: lost SDU unexpectedly contains payload")
            after_stop = (last_transmitted_seq is not None
                          and 0 < ((seq - last_transmitted_seq) & 0xFFFF) < 0x8000)
            window = "after_cutoff" if after_stop else "at_or_before_cutoff" if last_transmitted_seq is not None else "unsplit"
            counts[f"{window}_status_{status}"] += 1
            frame_index = None
            if valid:
                if data not in frames or not (native or stream) and data != frames[seq % 4]:
                    raise ValueError(f"line {number}: fixture mismatch at sequence {seq}")
                frame_index = frames.index(data)
                if native and matched_indices and frame_index <= matched_indices[-1]:
                    raise ValueError(f"line {number}: native fixture repeated/reordered")
                if stream and previous_valid_position is not None:
                    sequence_delta = sequence_position - previous_valid_position
                    expected_index = (matched_indices[-1] + sequence_delta) % 20
                    if frame_index != expected_index:
                        if not (allow_underrun and lost_since_valid
                                and frame_index == (matched_indices[-1] + 1) % 20):
                            raise ValueError(f"line {number}: stream fixture phase changed across RF ISO sequences")
                        inferred_padding.append({"previous_valid_sequence": previous_valid_seq,
                                                 "next_valid_sequence": seq,
                                                 "previous_fixture_index": matched_indices[-1],
                                                 "next_fixture_index": frame_index,
                                                 "recorded_lost_sdus": lost_since_valid,
                                                 "inferred_padding_slots": sequence_delta - 1})
                if gap_since_valid:
                    recoveries.append({"previous_valid_sequence": previous_valid_seq,
                                       "next_valid_sequence": seq,
                                       "previous_fixture_index": matched_indices[-1],
                                       "next_fixture_index": frame_index})
                gap_since_valid = False
                lost_since_valid = 0
                previous_valid_seq = seq
                previous_valid_position = sequence_position
                matched_indices.append(frame_index)
                frame_counts[frame_index] += 1
                output.extend(data)
            else:
                gap_since_valid |= previous_valid_seq is not None
                lost_since_valid += status == 2 and previous_valid_seq is not None
            packets.append({"count": count, "sequence": seq, "status": status,
                            "valid": valid, "window": window, "fixture_index": frame_index})
    if not (periodic and big and ready and output):
        raise ValueError("capture lacks successful periodic/BIG/ISO reception")
    if require_discovery and (not discovered or preconfigured):
        raise ValueError("capture does not prove scan-derived host discovery")
    if native and (len(matched_indices) <= 4 or not recoveries):
        raise ValueError("native proof needs >4 distinct received SDUs and reception after a gap")
    if stream and (len(matched_indices) < 40 or any(frame_counts[i] < 2 for i in range(20))):
        raise ValueError("stream proof needs >=40 valid SDUs and >=2 observations of every fixture frame")
    return {"valid_sdus": len(output) // 26, "logged_sdus": len(packets),
            "frame_counts": [frame_counts[i] for i in range(frame_total)],
            "fixture_mode": "cyclic-stream" if stream else "native-once" if native else "four-frame-repeat",
            "absolute_host_cycle": "ambiguous for repeated fixture" if stream else None,
            "allow_underrun": allow_underrun,
            "inferred_padding_recoveries": inferred_padding,
            "inferred_padding_slots": sum(gap["inferred_padding_slots"] for gap in inferred_padding),
            "padding_inference": "assumes adjacent host frames; repeated cycles remain ambiguous" if allow_underrun else None,
            "scan_derived_discovery": discovered and not preconfigured,
            "matched_fixture_indices": matched_indices,
            "unobserved_fixture_indices": [i for i in range(frame_total) if not frame_counts[i]],
            "initial_unobserved_fixture_indices": list(range(matched_indices[0])) if native else [],
            "recoveries_after_gap": recoveries,
            "first_sequence": packets[0]["sequence"], "last_sequence": packets[-1]["sequence"],
            "big_sync_lost": ended, "last_transmitted_sequence": last_transmitted_seq,
            "cutoff_source": "caller-supplied; not measured from receiver timestamps",
            "fixture_sha256": hashlib.sha256(fixture).hexdigest(),
            "counts": dict(sorted(counts.items())), "packets": packets}, bytes(output)


def self_check():
    fixture = b"".join(bytes([i]) * 26 for i in range(4))
    setup = ["periodic-established status: Success, adv_sid: 3, adv_addr_kind: AddrKind(1), "
             "adv_addr: BdAddr([1, 2, 3, 4, 5, 198]), adv_phy: Le1M",
             "periodic-data status=Complete data=[" + BASE.hex(" ") + "]",
             "biginfo num_bis: 1, nse: 1, iso_interval: 8, bn: 1, pto: 0, irc: 1, "
             "max_pdu: 26, max_sdu: 26, sdu_interval: ExtDuration([16, 39, 0]), "
             "phy: Le1M, is_framed: false, is_encrypted: false",
             "big-established status: Success,", "iso-ready handle=0"]
    records = ["iso count=1 seq=65535 status=0 len=26 valid=true data=[" + (bytes([3]) * 26).hex(" ") + "]",
               "iso count=2 seq=0 status=2 len=0 valid=false data=[]",
               "iso count=4 seq=2 status=0 len=26 valid=true data=[" + (bytes([2]) * 26).hex(" ") + "]",
               "iso count=5 seq=3 status=2 len=0 valid=false data=[]"]
    capture = "\n".join(PREFIX + line for line in setup + records)
    result, data = check(capture, fixture, 2)
    assert len(data) == 52 and result["valid_sdus"] == 2
    assert result["counts"]["at_or_before_cutoff_status_2"] == 1
    assert result["counts"]["after_cutoff_status_2"] == 1
    assert result["counts"]["missing_log_records"] == result["counts"]["sequence_gaps"] == 1
    discovery_capture = PREFIX + "discovered rssi=-30 data=[06, 16, 52, 18, 56, 34, 12]\n" + capture
    assert check(discovery_capture, fixture, 2, require_discovery=True)[0]["scan_derived_discovery"]
    try:
        check(capture, fixture, require_discovery=True)
    except ValueError:
        pass
    else:
        raise AssertionError("missing discovery accepted")
    for bad in (capture.replace("status: Success,", "status: Error,"),
                capture.replace("seq=65535", "seq=65534"),
                capture.replace("seq=3", "seq=2"),
                capture.replace("valid=true", "valid=false"),
                capture.replace("status=0 len=26 valid=true", "status=0 len=25 valid=false"),
                capture.replace("status=0 len=26 valid=true", "status=1 len=25 valid=false"),
                capture.replace("len=0", "len=1")):
        try:
            check(bad, fixture, 2)
        except ValueError:
            pass
        else:
            raise AssertionError("malformed capture accepted")
    fixture = b"".join(bytes([i]) * 26 for i in range(20))
    records = []
    for count, seq, index in ((1, 0, 2), (2, 1, 3), (3, 2, None),
                              (4, 3, 4), (5, 4, 5), (6, 5, 7)):
        suffix = ("status=2 len=0 valid=false data=[]" if index is None else
                  "status=0 len=26 valid=true data=[" + (bytes([index]) * 26).hex(" ") + "]")
        records.append(f"iso count={count} seq={seq} {suffix}")
    capture = "\n".join(PREFIX + line for line in setup + records)
    result, data = check(capture, fixture, native=True)
    assert len(data) == 130 and result["matched_fixture_indices"] == [2, 3, 4, 5, 7]
    assert result["initial_unobserved_fixture_indices"] == [0, 1]
    assert result["recoveries_after_gap"][0]["next_fixture_index"] == 4
    no_gap = "\n".join(PREFIX + line for line in setup + [
        f"iso count={i + 1} seq={i} status=0 len=26 valid=true data=["
        + (bytes([i + 2]) * 26).hex(" ") + "]" for i in range(6)])
    for bad in (capture.replace((bytes([7]) * 26).hex(" "), (bytes([2]) * 26).hex(" ")),
                "\n".join(capture.splitlines()[:-1]),
                no_gap,
                capture.replace("status=2 len=0 valid=false data=[]",
                                "status=0 len=26 valid=true data=[" + (bytes([19]) * 26).hex(" ") + "]")):
        try:
            check(bad, fixture, native=True)
        except ValueError:
            pass
        else:
            raise AssertionError("native proof without order/capacity/recovery accepted")
    records = []
    for i in range(61):
        seq = (65520 + i) & 0xFFFF
        suffix = ("status=2 len=0 valid=false data=[]" if i == 25 else
                  "status=0 len=26 valid=true data=[" + (bytes([(7 + i) % 20]) * 26).hex(" ") + "]")
        records.append(f"iso count={i + 1} seq={seq} {suffix}")
    capture = "\n".join(PREFIX + line for line in setup + records)
    result, _ = check(capture, fixture, stream=True)
    assert result["valid_sdus"] == 60 and min(result["frame_counts"]) >= 2
    assert result["recoveries_after_gap"] and result["absolute_host_cycle"]
    for bad in (capture.replace((bytes([7]) * 26).hex(" "), (bytes([8]) * 26).hex(" ")),
                "\n".join(capture.splitlines()[:-25])):
        try:
            check(bad, fixture, stream=True)
        except ValueError:
            pass
        else:
            raise AssertionError("wrong/short cyclic stream accepted")
    records = []
    for i in range(63):
        seq = (65520 + i) & 0xFFFF
        index = (7 + i - (3 if i >= 18 else 0)) % 20
        suffix = ("status=2 len=0 valid=false data=[]" if 15 <= i < 18 else
                  "status=0 len=26 valid=true data=[" + (bytes([index]) * 26).hex(" ") + "]")
        records.append(f"iso count={i + 1} seq={seq} {suffix}")
    underrun = "\n".join(PREFIX + line for line in setup + records)
    result, _ = check(underrun, fixture, stream=True, allow_underrun=True)
    assert result["valid_sdus"] == 60 and result["inferred_padding_slots"] == 3
    assert result["inferred_padding_recoveries"] == [{
        "previous_valid_sequence": 65534, "next_valid_sequence": 2,
        "previous_fixture_index": 1, "next_fixture_index": 2,
        "recorded_lost_sdus": 3, "inferred_padding_slots": 3}]
    assert result["recoveries_after_gap"] and result["absolute_host_cycle"]
    no_recorded_loss = "\n".join(line for line in underrun.splitlines() if "status=2" not in line)
    reordered = underrun.replace(PREFIX + records[18], PREFIX + records[18].replace(
        (bytes([2]) * 26).hex(" "), (bytes([3]) * 26).hex(" ")))
    no_gap_phase_change = underrun.replace(PREFIX + records[5], PREFIX + records[5].replace(
        (bytes([12]) * 26).hex(" "), (bytes([13]) * 26).hex(" ")))
    for bad, options in ((underrun, {"stream": True}),
                         (underrun, {"allow_underrun": True}),
                         (no_recorded_loss, {"stream": True, "allow_underrun": True}),
                         (reordered, {"stream": True, "allow_underrun": True}),
                         (no_gap_phase_change, {"stream": True, "allow_underrun": True})):
        try:
            check(bad, fixture, **options)
        except ValueError:
            pass
        else:
            raise AssertionError("underrun without opt-in/recorded loss/adjacent host frame accepted")
    assert not check(capture, fixture, stream=True, allow_underrun=True)[0]["inferred_padding_recoveries"]
    print("SDC checker self-check: sync, fixture modes, RF wrap/gaps, cutoff and strict/underrun recovery pass")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("capture", type=Path, nargs="?")
    parser.add_argument("--fixture", type=Path)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--native-fixture", action="store_true",
                      help="20 distinct SDUs sent once; require >4 matches and gap recovery")
    mode.add_argument("--stream-fixture", action="store_true",
                      help="20 cyclic SDUs; require >=40 valid with consistent RF sequence phase")
    parser.add_argument("--allow-underrun", action="store_true",
                        help="stream only: allow phase shifts across recorded loss to the next fixture frame")
    parser.add_argument("--require-discovery", action="store_true",
                        help="require matching scan report before periodic sync and no prearming")
    parser.add_argument("--last-transmitted-seq", type=int,
                        help="caller-supplied cutoff; establish exact TX stop independently")
    parser.add_argument("--output", type=Path, help="new directory for received SDUs, metadata and decoded PCM/WAV")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_check()
        return
    if args.capture is None or args.fixture is None:
        parser.error("capture and --fixture required")
    if args.allow_underrun and not args.stream_fixture:
        parser.error("--allow-underrun requires --stream-fixture")
    if args.last_transmitted_seq is not None and not 0 <= args.last_transmitted_seq <= 65535:
        parser.error("--last-transmitted-seq must fit u16")
    result, received = check(args.capture.read_text(), args.fixture.read_bytes(),
                             args.last_transmitted_seq, args.native_fixture, args.require_discovery,
                             args.stream_fixture, args.allow_underrun)
    if args.output:
        if args.output.exists():
            raise ValueError("output directory already exists; choose a new path")
        with tempfile.TemporaryDirectory(prefix="cyw43-sdc-") as temporary:
            directory = Path(temporary)
            (directory / "received.lc3").write_bytes(received)
            subprocess.run(["cargo", "run", "--quiet", "--features", "lc3", "--example", "radio_audio", "--",
                            "decode", str(directory / "received.lc3"), str(directory / "received.s16le")],
                           cwd=ROOT, check=True)
            pcm = (directory / "received.s16le").read_bytes()
            assert len(pcm) == result["valid_sdus"] * 80 * 2
            with wave.open(str(directory / "received.wav"), "wb") as wav:
                wav.setparams((1, 2, 8000, 0, "NONE", "not compressed"))
                wav.writeframes(pcm)
            result["decoded_samples"] = len(pcm) // 2
            result["output_omits_lost_sdus"] = True
            (directory / "capture.json").write_text(json.dumps(result, indent=2) + "\n")
            args.output.mkdir(parents=True)
            for source in directory.iterdir():
                (args.output / source.name).write_bytes(source.read_bytes())
    result.pop("packets")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
