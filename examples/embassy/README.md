# Embassy runtime installation

`runtime.rs` installs the experimental 4096-byte native ISO image through
Embassy CYW43 0.7 before its runner starts. `install` verifies stock version,
lookup prefix, task callback, DTM table, idle event and zero scratch; allocates
inside the controller task; checks every memory write; publishes native ISO
and credit callbacks; and verifies LE Read Buffer Size V2. `Installed::start`
arms the experimental discovery/BIS radio. Handle `0042` accepts complete
26-byte LC3 SDUs, with four HCI packet credits. This does not implement
standard HCI Create BIG/CIS negotiation.

The guarded boot image retains the signed `.0065` records and appends 112 zero
bytes at controller address `0xd0f90`; its SHA256 is
`b3c0c99774135c32beea47af847838077a1f0b7a6be661a68ddcd3641362ad7d`.
Raw HCI version must identify Infineon `0x0131`, subversion `0x2310`. BTstack's
`hci.c` rewrites Infineon to Broadcom `0x000f` before delivering that event;
the Embassy transport does not. SPI F1 response padding is 16 bytes, matching
Pico SDK. Four bytes returns stale controller-aperture words.
Bulk memory transfers set the 32-bit access flag and restore the backplane
window to CHIPCOMMON after each call. The patched PIO driver arms RX DMA before
sending the read command, preventing the 16-byte delay from filling the RX FIFO.
The minimal transport passed the 20-SDU live diagnostic, including all 20 RF
completions and credits, underrun recovery, and hook restoration.

Stock HCI RAM commands are gated after protected startup; see
`../../analysis/hci-ram-installer.md`. The small retained patch adds four
startup methods to the driver: checked memory read/write, direct HCI command
exchange, and a one-packet Bluetooth pump for the live probe. Existing public
`BtDriver` still serializes ISO and decodes events. No second SPI owner exists.

## Build and checks

From repository root:

```sh
cargo fetch --manifest-path examples/embassy-pico/Cargo.toml
python3 examples/embassy/prepare-runtime.py
cargo check --manifest-path examples/embassy/Cargo.toml --features runtime-installer \
  --target thumbv6m-none-eabi \
  --config "patch.crates-io.cyw43.path='$PWD/work/embassy-runtime/cyw43-0.7.0'"
rustc --edition 2024 --test --cfg 'feature="bluetooth"' \
  examples/embassy/check-runtime-driver.rs -o work/embassy-runtime/check-driver
work/embassy-runtime/check-driver
```

Preparation copies the crates.io `cyw43-0.7.0.crate` cache archive into `work`,
verifying archive SHA-256
`033e856bf6f38c96fee3e3f398cceff0ca2c2fae5db255bcd23495216c7047da`.
Its upstream source revision is `50c6aac5b9b3b0af18f5a7d61725cc8e4d1f7acb`.
Only the generated copy receives `cyw43-runtime.patch` and `driver_runtime.rs`;
installed crates and sibling checkouts remain untouched. Firmware blobs come
from the current checked-in C probe arrays, so rerun preparation after a
controller image change. The source crate license remains MIT OR Apache-2.0.

Preparation also verifies `cyw43-pio-0.10.0.crate` against SHA256
`768b74f36be804f7b1020938e711d91c0ab5a847cf620d3756245ae7e641e1e2`
and applies `cyw43-pio-runtime.patch`. Pico builds need both overrides:

```sh
DEFMT_LOG=info DEFMT_RTT_BUFFER_SIZE=32768 cargo build --release \
  --manifest-path examples/embassy-pico/Cargo.toml --target thumbv6m-none-eabi \
  --config "patch.crates-io.cyw43.path='$PWD/work/embassy-runtime/cyw43-0.7.0'" \
  --config "patch.crates-io.cyw43-pio.path='$PWD/work/embassy-runtime/cyw43-pio-iso-0.10.0'"
```

## Lifetime and failure

Use the returned runner exclusively: no concurrent Bluetooth host, Wi-Fi
control, or `Runner::run` task during installation. Afterwards either start
the normal runner or retain it and use the probe's Bluetooth-only pump.
The pump publishes at most one RX packet per call; consume each successful
result through `BtDriver::read` before pumping again.

Use raw `BtDriver` traffic after starting; normal TrouBLE host initialization
sends HCI Reset and stops the radio. It is unsupported for this startup-only API.

Any failed or cancelled operation requires chip restart and firmware reload;
HCI Reset alone does not reclaim SRAM or reliably remove hooks. Rollback checks
hook ownership and restores transport, lookup, and DTM table; allocations
and scratch persist until reset. Call `Installed::restore` only while still
owning the runner. Once `Runner::run` consumes it, reset is the available
recovery boundary. No arena rewind or signed-configuration modification occurs.

The released driver's SPI/backplane API does not report transfer errors;
this extension checks write readback and HCI envelopes, but retains upstream
SPI assertion/panic behavior. Its command wait polls 500 times with 1-ms
delays; external reset/watchdog remains necessary for a stuck underlying bus.
Live validation belongs to `../embassy-pico`, not these host checks.

## Isolated Embassy branch

The sibling `embassy-cyw43-iso` branch carries native `cyw43` 0.6.0 support under
`bluetooth-iso`, a private checked seam, fixed continuous-discovery blob, and
public `cyw43::iso::{install, Installed}` lifecycle. It contains no audio app or
codec. It references blobs built in `cyw43_iso/work/iso-blob-continuous` directly;
no copies live in Embassy. The separate `../rp2040_audio` application exercises
the normal runner:

```sh
cd ../rp2040_audio
cargo build --release --locked
```

The diagnostic exercises real `Runner::run` and raw `BtDriver`, sends 1000 SDUs
with 80-ms and 250-ms pauses, and validates credit conservation. It never calls
the startup-only pump. Final `EMBASSY_NATIVE done ... pass=true` means 1000 sent
and credited; over-air decode still needs a standard receiver. Every run ends
with external chip restart and stock firmware restoration by the hardware owner.

The live normal-runner capture in
`work/embassy-normal-runner-discovery-sdc-probe` passed 1000 sent/credited SDUs.
Nordic SDC discovered and synchronized to the stream, reassembling 950 exact SDUs
across all 20 repeating fixtures; reception resumed after the 250-ms pause.
Repeated fixtures do not establish absolute host-frame identity.
No LC3 decoding was tested. Both test boards were restored to stock firmware.
The RP2350 application is compile-checked only; live evidence uses RP2040.
