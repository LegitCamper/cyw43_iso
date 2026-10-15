#!/usr/bin/env python3
"""Check bounded RP2040 LC3 timing capture; missed deadlines are measured results."""
import argparse
import json
from pathlib import Path
import re


def check(text):
    starts = re.findall(r"LC3_BENCH start ([^\n]+)", text)
    results = re.findall(r"LC3_BENCH result ([^\n]+)", text)
    rows = [tuple(map(int, row)) for row in re.findall(
        r"LC3_BENCH frame=(\d+) encode_us=(\d+) decode_us=(\d+)", text)]
    assert len(starts) == len(results) == 1 and text.count("LC3_BENCH done") == 1
    start = dict(re.findall(r"(\w+)=(\w+)", starts[0]))
    result = dict(re.findall(r"(\w+)=(\w+)", results[0]))
    assert int(start["clock_hz"]) == 125_000_000 and start["rom_float"] in ("true", "false")
    for name, value in (("rate", 8000), ("samples", 80), ("octets", 26), ("frames", 20), ("deadline_us", 10000)):
        assert int(start[name]) == value, name
    assert [row[0] for row in rows] == list(range(20))
    assert all(encode > 0 and decode > 0 for _, encode, decode in rows)
    encode = [row[1] for row in rows]
    decode = [row[2] for row in rows]
    combined = [e + d for e, d in zip(encode, decode)]
    expected = {"frames": 20, "encode_max_us": max(encode), "decode_max_us": max(decode),
                "combined_max_us": max(combined), "encode_total_us": sum(encode),
                "decode_total_us": sum(decode), "encode_misses": sum(t > 10000 for t in encode),
                "decode_misses": sum(t > 10000 for t in decode),
                "combined_misses": sum(t > 10000 for t in combined)}
    for name, value in expected.items():
        assert int(result[name]) == value, (name, result[name], value)
    assert 1000 < int(result["peak"]) <= 32768
    assert 0 < int(result["nonzero_samples"]) <= 1600
    assert int(result["plc_us"]) > 0 and int(result["timer_overhead_us"]) >= 0
    assert 0 <= int(result["fixture_fnv32"], 16) <= 0xFFFFFFFF
    return {**start, **result,
            "measured_encode_within_10ms": max(encode) <= 10000,
            "measured_decode_within_10ms": max(decode) <= 10000,
            "measured_combined_within_10ms": max(combined) <= 10000,
            "scope": "20 fixture frames, cold first frame included, no concurrent radio or wrapper"}


def self_test():
    capture = "LC3_BENCH start clock_hz=125000000 rate=8000 samples=80 octets=26 frames=20 deadline_us=10000 workspace_bytes=10000 rom_float=true\n"
    capture += "".join(f"LC3_BENCH frame={i} encode_us=6000 decode_us=5000\n" for i in range(20))
    capture += "LC3_BENCH result frames=20 encode_max_us=6000 decode_max_us=5000 combined_max_us=11000 encode_misses=0 decode_misses=0 combined_misses=20 plc_us=100 timer_overhead_us=1 peak=6000 nonzero_samples=1500 fixture_fnv32=1234abcd encode_total_us=120000 decode_total_us=100000\nLC3_BENCH done\n"
    result = check(capture)
    assert result["measured_encode_within_10ms"] and not result["measured_combined_within_10ms"]
    for invalid in (capture.replace("combined_misses=20", "combined_misses=0"),
                    capture.replace("LC3_BENCH frame=10 ", "LC3_BENCH frame=9 "),
                    capture.replace("LC3_BENCH done", "")):
        try:
            check(invalid)
        except AssertionError:
            pass
        else:
            raise AssertionError("inconsistent/incomplete benchmark accepted")
    print("LC3 benchmark capture self-check passes; combined budget checked separately")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("capture", type=Path, nargs="?")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
    elif args.capture:
        print(json.dumps(check(args.capture.read_text()), indent=2))
    else:
        parser.error("capture or --self-test required")
