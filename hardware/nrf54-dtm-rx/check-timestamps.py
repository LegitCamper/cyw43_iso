#!/usr/bin/env python3
"""Check captured RADIO intervals; no device access or lossless-capture claim."""
import argparse
from pathlib import Path
import re

MASK = (1 << 32) - 1


def channel(counter):
    identifier = 0xCF58
    value = counter ^ identifier
    for _ in range(3):
        low = int(f"{value & 255:08b}"[::-1], 2)
        high = int(f"{value >> 8:08b}"[::-1], 2)
        value = (17 * (low | (high << 8)) + identifier) & 65535
    return (value ^ identifier) % 37


def check_hopping(text, minimum):
    assert "hopping_desync" not in text, "receiver lost synchronization"
    records = re.findall(
        r"DTM_RX hop packet=(\d+) counter=(\d+) channel=(\d+) next_counter=(\d+) next_channel=(\d+)", text
    )
    assert len(records) >= minimum + 1, "too few hopping records"
    for expected, record in enumerate(records):
        packet, counter, current, following, next_channel = map(int, record)
        assert packet == expected + 1 and counter == expected & 65535, "missing/reordered hop"
        assert following == (counter + 1) & 65535
        assert current == channel(counter) and next_channel == channel(following), "CSA#2 mismatch"
    return len(records)


def check(text, interval, tolerance, minimum):
    packets = {
        int(number): bytes(int(value.strip(), 16) for value in payload.split(","))
        for number, payload in re.findall(
            r"DTM_RX packet=(\d+) crc_ok=true .*?raw=\[([^\]]+)\]", text
        )
    }
    samples = re.findall(
        r"DTM_RX timing packet=(\d+) valid=(true|false) first=(true|false) "
        r"address_us=(\d+) phyend_us=(\d+) delta_us=(\d+)", text
    )
    previous = None
    intervals = []
    for number, valid, first, address, phyend, delta in samples:
        number, address, phyend, delta = map(int, (number, address, phyend, delta))
        assert 0 <= address <= MASK and 0 <= phyend <= MASK and 0 <= delta <= MASK
        packet = packets.get(number)
        if valid != "true" or packet is None:
            previous = None
            continue
        assert len(packet) >= 2 and len(packet) == packet[1] + 2, "invalid raw length"
        duration = (phyend - address) & MASK
        # BLE 1M after ADDRESS: two-byte header, payload and three-byte CRC.
        assert abs(duration - (len(packet) + 3) * 8) <= 3, f"invalid capture duration {duration}"
        if previous is not None and number == previous[0] + 1 and first == "false":
            assert delta == (address - previous[1]) & MASK, "capture delta mismatch"
            assert abs(delta - interval) <= tolerance, f"observed interval {delta} us"
            intervals.append(delta)
        previous = (number, address)
    assert len(intervals) >= minimum, f"only {len(intervals)} usable consecutive pairs"
    return intervals


def self_test():
    assert [channel(counter) for counter in range(8)] == [1, 28, 5, 16, 29, 30, 5, 29]
    lines = []
    for index, address in enumerate((0xFFFFF000, 0x1710, 0x3E20), 1):
        lines.append(f"DTM_RX timing packet={index} valid=true first={'true' if index == 1 else 'false'} "
                     f"address_us={address} phyend_us={(address + 48) & MASK} delta_us={0 if index == 1 else 10000}")
        lines.append(f"DTM_RX packet={index} crc_ok=true rx_crc=0 raw=[00, 01, 42]")
    text = "\n".join(lines)
    assert check(text, 10000, 0, 2) == [10000, 10000]
    hops = "\n".join(f"DTM_RX hop packet={counter + 1} counter={counter} channel={channel(counter)} "
                     f"next_counter={counter + 1} next_channel={channel(counter + 1)}" for counter in range(3))
    assert check_hopping(hops, 2) == 3
    try:
        check_hopping(hops.replace("channel=1 ", "channel=2 "), 2)
    except AssertionError:
        pass
    else:
        raise AssertionError("invalid hopping accepted")
    for bad in (text.replace("delta_us=10000", "delta_us=9999"), text.replace("valid=true", "valid=false")):
        try:
            check(bad, 10000, 0, 2)
        except AssertionError:
            continue
        raise AssertionError("invalid capture accepted")
    print("Timestamp checker wrap and rejection tests pass.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("capture", type=Path, nargs="?")
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--hopping", action="store_true")
    parser.add_argument("--interval-us", type=int, default=10000)
    parser.add_argument("--tolerance-us", type=int, default=10)
    parser.add_argument("--min-pairs", type=int, default=8)
    args = parser.parse_args()
    if args.self_test:
        self_test()
    if args.capture:
        if args.interval_us <= 0 or args.tolerance_us < 0 or args.min_pairs < 1:
            parser.error("interval/min-pairs must be positive; tolerance must be nonnegative")
        text = args.capture.read_text()
        values = check(text, args.interval_us, args.tolerance_us, args.min_pairs)
        if args.hopping:
            print(f"CSA#2 hopping records pass: packets={check_hopping(text, args.min_pairs)}.")
        print(f"Captured RADIO intervals pass: pairs={len(values)} min_us={min(values)} max_us={max(values)}.")
    elif not args.self_test:
        parser.error("capture or --self-test required")
