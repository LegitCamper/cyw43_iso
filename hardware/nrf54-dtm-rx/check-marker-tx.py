#!/usr/bin/env python3
"""Check the bounded transmitter log; RF reception is checked separately."""
import argparse
from pathlib import Path
import re

CHANNEL_ID = 0xcf58

def csa2_first(counter):
    """First subevent channel for a full 37-channel map (Core Vol 6 Part B 4.5.8.3)."""
    value = counter ^ CHANNEL_ID
    for _ in range(3):
        value = (int(f"{value & 255:08b}"[::-1], 2)
                 | (int(f"{value >> 8:08b}"[::-1], 2) << 8))
        value = (17 * value + CHANNEL_ID) & 0xffff
    return (value ^ CHANNEL_ID) % 37


def check(text, offset, bis=False, length=31, bad_crc=False, counter=False, hop=None, lc3=False):
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
    if lc3:
        assert bis and length == 26 and not counter and hop is None
        audio = (Path(__file__).with_name("audio.lc3")).read_bytes()
        assert len(audio) == 520
        expected = audio[:26]
    assert payload == expected, "wrong marker payload"
    counter_config = "DTM_TX counter first=0 count=500 payload-offset=0 width=2 endian=little"
    assert text.count("DTM_TX counter ") == int(counter), "unexpected counter configuration"
    lc3_config = "DTM_TX lc3 frames=20 octets=26 sample-rate-hz=8000 frame-us=10000"
    assert text.count("DTM_TX lc3 ") == int(lc3), "unexpected LC3 configuration"
    if lc3:
        assert text.count(lc3_config) == 1
        assert text.index("DTM_TX ready") < text.index(lc3_config) < text.index("DTM_TX done")
    if counter:
        assert text.count(counter_config) == 1
        assert text.index("DTM_TX ready") < text.index(counter_config) < text.index("DTM_TX done")
    assert text.count("DTM_TX hop ") == int(hop is not None), "unexpected hop configuration"
    if hop is not None:
        assert counter, "hopping markers carry packet indices"
        first = csa2_first(hop)
        hop_config = (f"DTM_TX hop channel-id=cf58 map=full counter-offset={hop} "
                      f"first-channel={first} first-mhz={2402 + 2 * first}")
        assert text.count(hop_config) == 1, "wrong hop configuration"
        assert text.index("DTM_TX ready") < text.index(hop_config) < text.index("DTM_TX done")
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
        audio = Path(__file__).with_name("audio.lc3").read_bytes()
        lc3_text = iso_text.replace(
            ", ".join(f"{value:02x}" for value in range(offset, offset + 26)),
            ", ".join(f"{value:02x}" for value in audio[:26]),
        ).replace("DTM_TX done", "DTM_TX lc3 frames=20 octets=26 sample-rate-hz=8000 frame-us=10000\nDTM_TX done")
        check(lc3_text, offset, bis=True, length=26, lc3=True)
        for bad in (counter_text.replace("count=500", "count=499"), counter_text.replace("endian=little", "endian=big")):
            try:
                check(bad, offset, bis=True, length=26, counter=True)
            except AssertionError:
                continue
            raise AssertionError("accepted invalid counter configuration")
        for hop_offset in (0, 1):
            first = csa2_first(hop_offset)
            hop_text = counter_text.replace(
                "DTM_TX done",
                f"DTM_TX hop channel-id=cf58 map=full counter-offset={hop_offset} "
                f"first-channel={first} first-mhz={2402 + 2 * first}\nDTM_TX done")
            check(hop_text, offset, bis=True, length=26, counter=True, hop=hop_offset)
            # A hop capture must not pass as fixed-channel and vice versa.
            wrong = [counter_text,
                     hop_text.replace(f"first-channel={first}", f"first-channel={(first + 1) % 37}"),
                     hop_text.replace(f"counter-offset={hop_offset}", f"counter-offset={hop_offset + 1}"),
                     hop_text.replace("map=full", "map=partial")]
            for bad in wrong:
                try:
                    check(bad, offset, bis=True, length=26, counter=True, hop=hop_offset)
                except AssertionError:
                    continue
                raise AssertionError("accepted invalid hop capture")
            try:
                check(hop_text, offset, bis=True, length=26, counter=True)
            except AssertionError:
                pass
            else:
                raise AssertionError("hop capture accepted as fixed channel")
        assert csa2_first(0) != csa2_first(1), "offset control must change channel"
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
    parser.add_argument("--lc3", action="store_true", help="cycle the checked 20-frame LC3 fixture")
    parser.add_argument("--hop", type=int, choices=(0, 1), default=None,
                        help="CSA#2 hopping markers with this event-counter offset")
    args = parser.parse_args()
    self_check()
    if args.capture:
        counter = args.counter or args.hop is not None
        iso = args.iso or args.bad_crc or counter or args.lc3
        check(args.capture.read_text(), 128 if args.alternate else 0, args.bis or iso,
              26 if iso else 31, args.bad_crc, counter, args.hop, args.lc3)
    print("Bounded marker transmitter capture check passes")
