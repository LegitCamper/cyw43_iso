#!/usr/bin/env python3
"""Prepare isolated pinned Embassy dependency and controller images; no hardware."""
import argparse
import hashlib
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
OUTPUT = ROOT / "work/embassy-runtime"
HASH = "033e856bf6f38c96fee3e3f398cceff0ca2c2fae5db255bcd23495216c7047da"
PIO_HASH = "768b74f36be804f7b1020938e711d91c0ab5a847cf620d3756245ae7e641e1e2"


def prepare_crate(cargo, name, digest, patch, destination_name):
    archives = list((cargo / "registry/cache").glob(f"*/{name}.crate"))
    assert archives, "Run cargo fetch --manifest-path examples/embassy-pico/Cargo.toml first"
    archive = next((path for path in archives if hashlib.sha256(path.read_bytes()).hexdigest() == digest), None)
    assert archive, f"No cache archive matches pinned {name} checksum"
    with tempfile.TemporaryDirectory(dir=OUTPUT) as temporary:
        directory = Path(temporary)
        with tarfile.open(archive) as package:
            package.extractall(directory, filter="data")
        source = directory / name
        subprocess.run(["patch", "--batch", "--fuzz=0", "-p1", "-i", str(HERE / patch)], cwd=source, check=True)
        if name == "cyw43-0.7.0":
            shutil.copyfile(HERE / "driver_runtime.rs", source / "src/runtime_bootstrap.rs")
        destination = OUTPUT / destination_name
        if destination.exists():
            assert (destination / ".cyw43-iso-generated").read_text() == digest
            shutil.rmtree(destination)
        (source / ".cyw43-iso-generated").write_text(digest)
        shutil.move(source, destination)
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--firmware-dir", type=Path, help="Copy the pinned Wi-Fi and RP2040 NVRAM assets for the Pico probe")
    args = parser.parse_args()
    cargo = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    OUTPUT.mkdir(parents=True, exist_ok=True)
    destination = prepare_crate(cargo, "cyw43-0.7.0", HASH, "cyw43-runtime.patch", "cyw43-0.7.0")
    pio = prepare_crate(cargo, "cyw43-pio-0.10.0", PIO_HASH, "cyw43-pio-runtime.patch", "cyw43-pio-iso-0.10.0")
    text = (ROOT / "hardware/picow_runtime_probe.c").read_text()
    for symbol, filename, size in (("controller_native_iso", "native.bin", 4096), ("sram_allocator", "allocator.bin", 112), ("native_audio", "audio.bin", 520)):
        match = re.search(r"static const uint8_t " + symbol + r"\[\] = \{(.*?)\};", text, re.S)
        assert match, symbol
        blob = bytearray(int(value, 16) for value in re.findall(r"0x([0-9a-f]{2})\b", match[1]))
        assert len(blob) == size, (symbol, len(blob))
        if symbol == "sram_allocator":
            blob[56:58] = b"\x80\x50"  # mov.w r0,#4096; same guarded allocator.
        (OUTPUT / filename).write_bytes(blob)
        print(f"{filename}: {len(blob)} bytes sha256={hashlib.sha256(blob).hexdigest()}")
    if args.firmware_dir:
        for source_name, name, digest in (
            ("43439A0.bin", "wifi.bin", "5555e0261da2610a500d68c18d895cace0152bbefbf76f4aa683ebce77e3d7eb"),
            ("nvram_rp2040.bin", "nvram.bin", "4904bdbb0c937bd0ac2eb2a1d62f2da4dd90e32082384e02874e8d671b0f330d"),
        ):
            blob = (args.firmware_dir / source_name).read_bytes()
            assert hashlib.sha256(blob).hexdigest() == digest, source_name
            (OUTPUT / name).write_bytes(blob)
        assert hashlib.sha256((ROOT / "work/sdk0065.bin").read_bytes()).hexdigest() == "d974384d0aa8124d30084f30d13519a3ed3b9f759f7ddd6a34ef3dcf1441bcf7"
        assert hashlib.sha256((ROOT / "work/sram-exec-probe/zero-append.bin").read_bytes()).hexdigest() == "b3c0c99774135c32beea47af847838077a1f0b7a6be661a68ddcd3641362ad7d"
    print("Prepared hash-verified cyw43 0.7.0 with startup-only runtime seam.")
    print('cargo check --manifest-path examples/embassy/Cargo.toml --features runtime-installer --config \'patch.crates-io.cyw43.path="' + str(destination) + '"\'')
    print('Pico build also needs --config \'patch.crates-io.cyw43-pio.path="' + str(pio) + '"\'')


if __name__ == "__main__":
    main()
