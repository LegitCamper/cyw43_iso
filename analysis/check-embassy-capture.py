#!/usr/bin/env python3
"""Validate Embassy native ISO accounting, hook rollback, and stock restoration."""
import argparse
from pathlib import Path
import re


PREFIX = "EMBASSY_ISO "
FIELDS = ("sent", "accepted", "consumed", "rf", "reported", "credits",
          "underruns", "inflight", "blocked", "head", "tail")


def check(capture, stock, count=20):
    lines = [line.split(PREFIX, 1)[1] for line in capture.splitlines() if PREFIX in line]
    assert count > 8, "proof requires initial credits, pause, and resumed SDUs"
    assert not any("failed" in line or "=false" in line for line in lines), "failed checkpoint"
    assert lines.count("begin") == lines.count("initialized") == 1
    allocations = [int(match[1], 16) for line in lines
                   if (match := re.fullmatch(r"allocation=([0-9a-f]{8})", line))]
    assert len(allocations) == 1 and allocations[0] % 4 == 0
    assert 0x209bac <= allocations[0] <= 0x21d000
    assert lines.count("pause=80ms") == 1
    total = 0
    credit_positions = []
    for index, line in enumerate(lines):
        if match := re.fullmatch(r"credit=(\d+) total=(\d+)", line):
            credit, reported = map(int, match.groups())
            assert 0 < credit <= 4, "invalid credit batch"
            total += credit
            assert reported == total <= count, "duplicate, missing, or excess credit"
            credit_positions.append(index)
    assert total == count, "not all credits returned"
    counters = [(index, dict((key, int(value)) for key, value in re.findall(r"(\w+)=(\d+)", line)))
                for index, line in enumerate(lines) if line.startswith("sent=")]
    assert len(counters) == 1
    counter_position, values = counters[0]
    assert set(values) == set(FIELDS), "missing or unexpected accounting field"
    assert all(values[field] == count for field in FIELDS[:6]), "SDU lifecycle incomplete"
    assert values["underruns"] > 0 and values["inflight"] == values["blocked"] == 0
    assert 0 <= values["head"] == values["tail"] < 5, "queue not empty or corrupt"
    assert lines.count("restored=true pass=true") == 1, "hooks not restored"
    assert credit_positions[-1] < counter_position < lines.index("restored=true pass=true")
    assert any(index > lines.index("pause=80ms") for index in credit_positions), "no recovery after pause"
    assert stock.count("RESULT: controller ISO support absent") == 2, "stock capability passes missing"
    assert len(re.findall(r"LMP subversion\s+0x2310", stock)) == 2, "stock version not restored"


def check_normal_runner(capture, stock, count=1000):
    prefix = "EMBASSY_NATIVE "
    lines = [line.split(prefix, 1)[1] for line in capture.splitlines() if prefix in line]
    assert count > 500 and lines.count("begin") == 1
    assert not any("failed" in line or "=false" in line for line in lines)
    allocations = [int(match[1], 16) for line in lines
                   if (match := re.fullmatch(r"allocation=([0-9a-f]{8})", line))]
    assert len(allocations) == 1 and allocations[0] % 4 == 0
    assert 0x209bac <= allocations[0] <= 0x21d000
    assert lines.count("pause=80ms") == lines.count("pause=250ms") == 1
    assert lines.index("pause=80ms") < lines.index("pause=250ms")
    previous_sent = previous_credits = 0
    progress = []
    for index, line in enumerate(lines):
        if match := re.fullmatch(r"sent=(\d+) credits=(\d+)", line):
            sent, credits = map(int, match.groups())
            assert previous_sent <= sent <= count and previous_credits < credits <= sent
            assert sent - credits <= 4, "host exceeded controller credit capacity"
            previous_sent, previous_credits = sent, credits
            progress.append(index)
    assert len(progress) >= 2 and progress[0] < lines.index("pause=250ms") < progress[-1]
    assert previous_sent == previous_credits == count, "final progress does not drain host credits"
    # Normal Runner owns the bus: only external reset/stock checks prove teardown.
    done = f"done sent={count} credits={count} pass=true restart_required=true"
    assert lines.count(done) == 1 and progress[-1] < lines.index(done)
    assert stock.count("RESULT: controller ISO support absent") == 2
    assert len(re.findall(r"LMP subversion\s+0x2310", stock)) == 2


def self_check():
    lines = ["begin", "initialized", "allocation=00217e78"]
    for total in range(1, 21):
        if total == 5:
            lines.append("pause=80ms")
        lines.append(f"credit=1 total={total}")
    lines += ["sent=20 accepted=20 consumed=20 rf=20 reported=20 credits=20 "
              "underruns=4 inflight=0 blocked=0 head=0 tail=0", "restored=true pass=true"]
    capture = "\n".join(PREFIX + line for line in lines)
    stock = ("LMP subversion 0x2310\nRESULT: controller ISO support absent\n") * 2
    check(capture, stock)
    for old, new in (("allocation=00217e78", "allocation=0021d004"),
                     ("total=10", "total=9"), ("rf=20", "rf=19"),
                     ("inflight=0", "inflight=1"), ("blocked=0", "blocked=1"),
                     ("tail=0", "tail=1"), ("underruns=4", "underruns=0"),
                     ("restored=true", "restored=false"), ("pass=true", "pass=false")):
        try:
            check(capture.replace(old, new), stock)
        except AssertionError:
            pass
        else:
            raise AssertionError(f"invalid capture accepted: {old}")
    try:
        check(capture, stock.replace("0x2310", "0x2311"))
    except AssertionError:
        pass
    else:
        raise AssertionError("wrong restored firmware accepted")
    normal = "\n".join("EMBASSY_NATIVE " + line for line in (
        "begin", "allocation=00217e78", "pause=80ms", "sent=103 credits=100",
        "pause=250ms", "sent=1000 credits=1000",
        "done sent=1000 credits=1000 pass=true restart_required=true"))
    check_normal_runner(normal, stock)
    for old, new in (("credits=1000", "credits=999"), ("sent=103", "sent=105"),
                     ("pause=250ms", "pause=251ms"), ("restart_required=true", "restart_required=false")):
        try:
            check_normal_runner(normal.replace(old, new), stock)
        except AssertionError:
            pass
        else:
            raise AssertionError(f"invalid normal-runner capture accepted: {old}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("capture", type=Path, nargs="?")
    parser.add_argument("stock", type=Path, nargs="?")
    parser.add_argument("--count", type=int)
    parser.add_argument("--normal-runner", action="store_true",
                        help="validate normal Runner credits and mandatory external reset, without queue/hook readback claims")
    args = parser.parse_args()
    self_check()
    if (args.capture is None) != (args.stock is None):
        parser.error("capture and stock paths must be supplied together")
    if args.capture is not None:
        count = args.count if args.count is not None else 1000 if args.normal_runner else 20
        checker = check_normal_runner if args.normal_runner else check
        checker(args.capture.read_text(), args.stock.read_text(), count)
        detail = "normal Runner credits and external stock reset" if args.normal_runner else "empty queue, hooks and stock restored"
        print(f"Embassy ISO capture passes: {count} completed/credited SDUs, {detail}")
    else:
        print("Embassy ISO capture checker self-check passes")
