#!/usr/bin/env python3
"""Check the bounded transmitter log; RF reception is checked separately."""
import argparse
from pathlib import Path
import re


def check(text, offset, bis=False, length=31, bad_crc=False, counter=False):
    assert "failed" not in text and "panicked" not in text
    config = ("mhz=2404 mode=BLE1M aa=718ebed6 crc_init=123401 whitening=true"
              if bis else "mhz=2402 mode=BLE1M aa=71764129 crc_init=555555 whitening=false")
    if bad_crc:
        assert bis and length == 26
        config = config.replace("123401", "123400")
    matches = re.findall(r"DTM_TX ready " + config + rf" header={length:02x}00 payload=\[([^]]+)\]", text)
    assert len(matches) == 1, "missing or repeated transmitter configuration"
    payload = bytes(int(value.strip(), 16) for value in matches[0].split(","))
    expected = bytes(range(offset, offset + length))
    if counter:
        assert bis and length == 26
        expected = b"\0\0" + expected[2:]
    assert payload == expected, "wrong marker payload"
    counter_config = "DTM_TX counter first=0 count=500 payload-offset=0 width=2 endian=little"
    assert text.count("DTM_TX counter ") == int(counter), "unexpected counter configuration"
    if counter:
        assert text.count(counter_config) == 1
        assert text.index("DTM_TX ready") < text.index(counter_config) < text.index("DTM_TX done")
    assert text.count("DTM_TX done sent=500 disabled=true") == 1
    assert text.index("DTM_TX ready") < text.index("DTM_TX done")


def self_check():
    for offset in (0, 128):
        text = (
            "DTM_TX ready mhz=2402 mode=BLE1M aa=71764129 crc_init=555555 "
            "whitening=false header=1f00 payload=["
            + ", ".join(f"{value:02x}" for value in range(offset, offset + 31))
            + "]\nDTM_TX done sent=500 disabled=true\n"
        )
        check(text, offset)
        bis_text = text.replace("2402", "2404").replace("71764129", "718ebed6").replace("555555", "123401").replace("whitening=false", "whitening=true")
        check(bis_text, offset, bis=True)
        iso_text = bis_text.replace("header=1f00", "header=1a00").replace(
            ", ".join(f"{value:02x}" for value in range(offset, offset + 31)),
            ", ".join(f"{value:02x}" for value in range(offset, offset + 26)))
        check(iso_text, offset, bis=True, length=26)
        check(iso_text.replace("123401", "123400"), offset, bis=True, length=26, bad_crc=True)
        counter_text = iso_text.replace(f"payload=[{offset:02x}, {offset + 1:02x}", "payload=[00, 00").replace(
            "DTM_TX done", "DTM_TX counter first=0 count=500 payload-offset=0 width=2 endian=little\nDTM_TX done")
        check(counter_text, offset, bis=True, length=26, counter=True)
        for bad in (counter_text.replace("count=500", "count=499"), counter_text.replace("endian=little", "endian=big")):
            try:
                check(bad, offset, bis=True, length=26, counter=True)
            except AssertionError:
                continue
            raise AssertionError("accepted invalid counter configuration")
        for wrong_config, bis in ((text, True), (bis_text, False)):
            try:
                check(wrong_config, offset, bis=bis)
            except AssertionError:
                continue
            raise AssertionError("accepted wrong radio identity")
        for bad in (text.replace("1f00", "1e00"), text.replace("500", "499"), text + "failed"):
            try:
                check(bad, offset)
            except AssertionError:
                continue
            raise AssertionError("accepted invalid transmitter capture")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("capture", nargs="?", type=Path)
    parser.add_argument("--alternate", action="store_true")
    parser.add_argument("--bis", action="store_true")
    parser.add_argument("--iso", action="store_true", help="26-byte BIS-identity marker")
    parser.add_argument("--bad-crc", action="store_true", help="26-byte marker with one CRC-init bit toggled")
    parser.add_argument("--counter", action="store_true", help="26-byte markers with LE u16 packet indices")
    args = parser.parse_args()
    self_check()
    if args.capture:
        iso = args.iso or args.bad_crc or args.counter
        check(args.capture.read_text(), 128 if args.alternate else 0, args.bis or iso, 26 if iso else 31, args.bad_crc, args.counter)
    print("Bounded marker transmitter capture check passes")
