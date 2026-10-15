#!/usr/bin/env python3
"""Replay discovery captures and independently check software CRC/radio tuning."""
import argparse
from pathlib import Path
import re
import runpy
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
CRC24 = runpy.run_path(str(ROOT / "analysis/check-controller-tx.py"))["crc24"]


def channel(address, counter, mapping):
    identifier = (address >> 16) ^ (address & 65535)
    value = counter ^ identifier
    for _ in range(3):
        permuted = int(f"{value & 255:08b}"[::-1], 2) | (int(f"{value >> 8:08b}"[::-1], 2) << 8)
        value = (17 * permuted + identifier) & 65535
    value ^= identifier
    enabled = [i for i in range(37) if mapping & (1 << i)]
    assert len(enabled) >= 2
    return value % 37 if value % 37 in enabled else enabled[(len(enabled) * value) >> 16]


def check(path):
    with tempfile.TemporaryDirectory(prefix="cyw43-discovery-") as directory:
        executable = Path(directory) / "replay"
        subprocess.run(["rustc", "--edition", "2024", "-A", "dead_code", str(Path(__file__).with_suffix(".rs")), "-o", str(executable)], check=True)
        subprocess.run([str(executable), str(path)], check=True)
    text = path.read_text()
    packets = {
        int(number): (bytes(int(byte.strip(), 16) for byte in raw.split(',')), int(crc, 16))
        for number, crc, raw in re.findall(r"DTM_RX packet=(\d+) crc_ok=true rx_crc=0x([0-9a-f]+) raw=\[([^]]+)\]", text)
    }
    rows = []
    for line in text.splitlines():
        if "DTM_RX discovery packet=" in line:
            rows.append({key: int(value, 16 if value.startswith('0x') else 10)
                         for key, value in re.findall(r"(\w+)=(0x[0-9a-f]+|\d+)", line)})
    assert len(rows) == len(packets), "missing discovery logs"
    expected_stage, address, crc_init, current_channel = 0, 0x8e89bed6, 0x555555, 37
    periodic_address = periodic_crc = mapping = counter = broadcast_id = 0
    bis_counter = 0
    le = lambda value: int.from_bytes(value, 'little')
    for number, row in enumerate(rows, 1):
        assert row['packet'] == number and row['stage'] == expected_stage
        assert row['aa'] == address and row['channel'] == current_channel, "wrong acquired radio identity/channel"
        raw, crc = packets[number]
        assert CRC24(raw, crc_init) == crc, f"software CRC mismatch packet {number}"
        if expected_stage == 0:
            current_channel = raw[6] & 63
            expected_stage = 1
        elif expected_stage == 1:
            sync = raw[12:30]
            periodic_address, periodic_crc = le(sync[9:13]), le(sync[13:16])
            mapping, counter = le(sync[4:9]) & ((1 << 37) - 1), le(sync[16:18])
            broadcast_id = le(raw[-3:])
            address, crc_init = periodic_address, periodic_crc
            current_channel = channel(address, counter, mapping)
            expected_stage = 2
        elif expected_stage == 2:
            big = raw[6:39]
            bis_counter = le(big[28:33]) & ((1 << 39) - 1)
            address, crc_init = le(big[13:17]) ^ 0xfd060000, (le(big[21:23]) << 8) | 1
            current_channel = channel(address, bis_counter & 65535, le(big[23:28]) & ((1 << 37) - 1))
            assert row['bis_counter'] == bis_counter & 65535
            expected_stage = 3
        else:
            counter = (counter + 1) & 65535
            address, crc_init = periodic_address, periodic_crc
            current_channel = channel(address, counter, mapping)
            expected_stage = 2
        assert row['next_aa'] == address and row['next_channel'] == current_channel, "wrong next radio identity/channel"
        assert row['periodic_counter'] == counter and row['broadcast_id'] == broadcast_id
    print(f"Discovery software CRC24 and parsed radio identities pass: {len(rows)} packets.")


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('capture', type=Path)
    check(parser.parse_args().capture.resolve())
