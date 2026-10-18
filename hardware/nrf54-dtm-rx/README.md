# nRF54L15 raw DTM receiver

Diagnostic receive-only firmware for nRF54L15 Connect Kit CPUAPP secure:
BLE 1M, 2402 MHz, access address `0x71764129`, CRC init `0x555555`,
CRC polynomial `0x065b`, whitening disabled. RTT/defmt prints S0, length,
raw payload and CRC status, including CRC-failed receptions. No HCI stack.

Build from repository root (does not touch a probe):

```sh
cargo build --manifest-path hardware/nrf54-dtm-rx/Cargo.toml \
  --release --target thumbv8m.main-none-eabihf
```

ELF: `hardware/nrf54-dtm-rx/target/thumbv8m.main-none-eabihf/release/nrf54-dtm-rx`.
Use this same ELF for RTT defmt decoding. Firmware never enables TX.

The separate `marker-tx` binary sends 500 fixed-channel packets, approximately
10 ms apart, then leaves RADIO disabled. Its payload is bytes `00..1e`;
feature `marker-alt` selects `80..9e`. Header is `00 1f`, with the same
DTM access address, CRC and disabled whitening as the receiver. Build with
`--bin marker-tx` (and optionally `--features marker-alt`). This bounded
source supports the controller RX snapshot experiment; it makes no precise
cadence claim. Check its RTT log using `python3
hardware/nrf54-dtm-rx/check-marker-tx.py capture.txt` (`--alternate` for the
second marker). Running the checker without a capture runs its self-check.

Feature `marker-bis` sends the same markers at 2404 MHz using access address
`0x718ebed6`, CRC init `0x123401`, and whitening for logical channel 0; check
with `--bis`. Feature `marker-iso` implies `marker-bis`, shortens the payload
to 26 bytes (`00..19` or `80..99`), and uses physical header `00 1a`; check
with `--iso`. These markers test bounded RX transport, without BIG synchronization.
Feature `marker-bad-crc` implies `marker-iso` and flips CRC init from `0x123401`
to `0x123400`, preserving the other radio settings and payload; check its
transmitter log with `--bad-crc`. Receiver evidence determines whether this
negative control is rejected.
Feature `marker-counter` implies `marker-iso` and replaces payload bytes 0..1
with the little-endian packet index, 0 through 499. Remaining bytes stay fixed;
index 0 is loaded before the ready log. Updates occur between completed radio
transmissions, with a compiler fence and DMB before TX. Check the configuration
with `--counter`; receiver capture must independently verify all indices.

Feature `marker-lc3` also implies `marker-iso` and cycles the checked 520-byte,
20-frame BAP 8_2 fixture (`audio.lc3`) at the same approximate 10-ms [NAME_b771624e1e25]
cadence. It is the fixed-channel source for `trouble_audio`'s experimental
`rp-pico-w/cyw43_iso_sink` example. The receiver decodes each 26-byte SDU and
blinks its onboard LED while audio arrives. Build and inspect its transmitter
log with:

```sh
cargo build --release --target thumbv8m.main-none-eabihf --bin marker-tx --features marker-lc3
python3 check-marker-tx.py capture.txt --lc3
```

This is a bounded raw-radio fixture, not a standard advertising, periodic-sync,
or BIG source. The fixture lets the CYW43439 fixed-channel LC3 receive and host
decode path be validated independently before BIGInfo synchronization exists.

`DTM_RX ready` confirms crystal/init completed; `waiting` reports RADIO state,
READY and ADDRESS events. Packet logs include both successful and failed CRCs.
Diagnostics use unfiltered `defmt::println!`, requiring no `DEFMT_LOG` build
setting. RTT uses nonblocking mode; unread logs can drop. RX pauses during
packet logging.

The `bis` feature selects a fixed raw BIS packet preset: BLE 1M, data channel
0 at 2404 MHz, access address `0x718ebed6`, CRC init `0x123401`, whitening
enabled for logical channel 0. nRF54L uses `DATAWHITE=0x00890040` (BLE
polynomial `0x89`, seed `0x40 | channel`), matching Zephyr's nRF54L HAL;
it does not expose the older `DATAWHITEIV` register. It accepts raw headers and payloads, including
LLID 0 and 31-byte packets. This is a packet diagnostic, without BIG discovery,
channel hopping, event synchronization, or ISOAL. Build into a separate target
directory to preserve the working default DTM ELF:

```sh
CARGO_TARGET_DIR=work/nrf54-bis-rx cargo build \
  --manifest-path hardware/nrf54-dtm-rx/Cargo.toml --features bis \
  --release --target thumbv8m.main-none-eabihf
```

Variant ELF: `work/nrf54-bis-rx/thumbv8m.main-none-eabihf/release/nrf54-dtm-rx`.
The ready log prints the actual compiled radio configuration.

The optional `timestamps` feature captures RADIO ADDRESS and PHYEND directly
through DPPIC10 channels 0/1 into TIMER10 CC0/CC1. TIMER10 runs at 1 MHz
(32 MHz base, prescaler 5), 32-bit free-running. Packet timing logs contain
hardware event captures and wrapping address-to-address deltas, not CPU
polling times. ADDRESS is an address-detection event, not packet-start time;
constant-format intervals remain comparable. Logging can still cause missed
packets; a longer observed gap is not proof of a transmitter scheduling gap.

```sh
CARGO_TARGET_DIR=work/nrf54-bis-timestamp-rx cargo build \
  --manifest-path hardware/nrf54-dtm-rx/Cargo.toml --features bis,timestamps \
  --release --target thumbv8m.main-none-eabihf
python3 hardware/nrf54-dtm-rx/check-timestamps.py receiver.txt \
  --interval-us 10000 --tolerance-us 10 --min-pairs 8
```

The checker requires CRC-valid packet logs, matching hardware durations and
at least the requested number of consecutive captured intervals. It handles
timer wrap, rejects stale/zero captures, and does not prove lossless reception.
Run its offline regression with `python3 hardware/nrf54-dtm-rx/check-timestamps.py --self-test`.

The `hopping` feature implies `bis,timestamps` and reuses the repository's
tested `IsoChannels` for NSE=1, all 37 data channels, event counter starting
at zero. Arm RX before starting TX. Each CRC-valid packet advances the
counter, retunes frequency and whitening while RADIO is disabled, then
rearms RX. CRC failure or an observed interval outside 10000 +/- 20 us
halts reception and logs `hopping_desync`; there is no packet-loss recovery
or BIG synchronization. Missing a packet can leave RX waiting on the old
channel until another transmission matches, which then fails the cadence
check. A first late acquisition cannot prove counter zero.

```sh
CARGO_TARGET_DIR=work/nrf54-hopping-rx cargo build \
  --manifest-path hardware/nrf54-dtm-rx/Cargo.toml --features hopping \
  --release --target thumbv8m.main-none-eabihf
```

ELF: `work/nrf54-hopping-rx/thumbv8m.main-none-eabihf/release/nrf54-dtm-rx`.
For access address `0x718ebed6`, channel ID is `0xcf58`; first channels are
`1, 28, 5, 16, 29, 30, 5, 29`. `DTM_RX hop` logs the received and next
counter/channel. Preserve earlier ELF snapshots for their matching defmt logs.
Add `--hopping` to the timestamp checker to require counter-zero acquisition,
contiguous logged counters, and independent full-map CSA#2 channel checks.

Hardware operations belong exclusively to the controller-radio agent. Probe
selection must include full serial, `--chip nRF54l15`, application core 0,
CMSIS-DAP/SWD. Existing `trouble_audio/hwtest/README.md` identifies Connect Kit
roles A=`820D9A5F0FC04A038AA34`, B=`820D9A5F0FDB93520E1DB`, both `2fe3:0204`.
Those are prior documented identities, not a fresh device enumeration. Preserve
existing firmware before replacing it; build alone has not tested reception.

Startup uses pinned Embassy `init`: its nRF54L path performs volatile TAMPC
configuration, resets FLPR by default, and applies FICR `TRIMCNF` entries to
vendor-selected addresses. No explicit nRF54L UICR write was found in that
branch; this is static source evidence, not proof of unchanged device state.
The hardware owner must preserve/compare UICR and restore the original image.

Register configuration follows the locally installed Nordic NCS
`nrf/samples/bluetooth/direct_test_mode/src/dtm.c` (`radio_init`, `radio_prepare`,
`clock_init`). nRF54L uses `PHYEND_DISABLE`, not the nRF52 `END_DISABLE` shortcut.
Peripherals and register fields come from Embassy's nRF54L15 PAC; no raw MMIO
addresses are invented. Embassy revision, memory layout, target and external
HF crystal follow existing `ble_audio_test/firmware/nrf54l15` and
`trouble_audio/examples/nRF54L15_Connect_Kit` projects.
## Discovery receiver

`--features discovery` starts on primary advertising channel 37, advertising AA
`0x8e89bed6`, CRCInit `0x555555`, selecting fixture SID 3 / DID `0xabc`.
Unrelated primary packets and failed CRCs are ignored before acquisition.
It parses received AuxPtr, SyncInfo, BIGInfo and
BASE; each next channel, access address, CRC, counter, and anchor comes from
those fields. It follows one cleartext, unframed BIS with NSE=BN=IRC=1 and equal
periodic/BIG intervals. LC3 configuration must describe one mono frame per SDU.
Unsupported extended-header fields, framing, overrides, loss, CRC failures,
invalid packets, or anchor violations halt acquisition with `discovery_error`.
This diagnostic is not a general broadcast scanner or production audio sink.

Build from this directory:

```sh
DEFMT_RTT_BUFFER_SIZE=32768 cargo build --release --target thumbv8m.main-none-eabihf --features discovery --target-dir ../../work/nrf54-discovery-rx
rustc --edition 2024 --test check-discovery.rs -o /tmp/check-discovery-tests
/tmp/check-discovery-tests
python3 check-discovery.py ../../work/controller-discovery-rf-probe/receiver.txt
```

Capture checker replays received packet bytes through the same parser, checks
hardware timestamps and CRC status, and requires at least eight BIS packets.
Python independently checks software CRC24 with each advertised initialization
value, parsed radio identities, and current/next CSA#2 channels against logs.
Unit checks include changed advertised identities/channels, truncated PDUs,
counter progression, and wrapping timestamps. Arm receiver before transmitter.
`discovery` takes precedence if combined with `hopping`; all logs remain
unfiltered, and radio stays receive-only.

