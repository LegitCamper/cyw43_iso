# CYW43439 ISO controller patch

This repository builds and validates experimental controller RAM patches for
ISO on CYW43439. The sibling `rp2040_audio` repository holds application tests
and audio logic. Embassy holds driver/installer patches and directly includes
the blobs built here; it does not maintain its own blob copy. The host-side
protocol modules here support controller development and testing.

Verified hardware path: `rp2040_audio` sends native HCI ISO through Embassy's
production runner, with four transmit credits and one unencrypted BIS. A
1,000-SDU run returns all credits; stock Nordic SDC discovers the broadcast
normally and receives 950 exact SDUs, including recovery across a 250-ms refill
pause. Both devices are restored afterwards. An earlier Pico SDK run also
demonstrates late joining roughly eight seconds into the stream. See
[`analysis/controller-native-iso.md`](analysis/controller-native-iso.md).

Build and export the checked blob:

```sh
python3 analysis/check-controller-tx.py --continuous --output-blob work/iso-blob-continuous
```

Output: `native.bin` (4096-byte resident patch), `allocator.bin` (112-byte guarded
bootstrap), `manifest.json` (ABI, offsets and SHA-256 hashes), and `audio.lc3`
(test fixture only). Building requires the GNU Arm toolchain, Cargo and the
previously captured clean ROM under `work/`. `--continuous` keeps discovery
advertisements running during streaming; `--native` selects the earlier variant
with discovery only at startup.

This is a runtime patch for checked `.0065` firmware, not a replacement stock
firmware file. Installation requires guarded allocation, verified backplane
writes and callback publication. Boot firmware must reserve 112 zero bytes at
`0x000d0f90`; the manifest identifies the tested `zero-append.bin` by hash.
Chip restart and firmware reload own reclamation. Current transport is
fixed to handle `0x0042`, complete timestamp-free 26-byte SDUs and a 10-ms ISO
interval. Standard BIG creation, CIS, encryption and general ISO scheduling are
not implemented. The stock DTM event owns the radio during this experiment;
normal concurrent controller operation is not established. Embassy host
integration is separate from blob generation, on branch `cyw43-iso` in the
sibling `embassy-cyw43-iso` worktree. Run the export command before building
that branch with `bluetooth-iso`. Its installer references
`cyw43_iso/work/iso-blob-continuous/{native,allocator}.bin` in the sibling repo.
Current synchronized radio support is BIS transmit; general CYW43 ISO receive remains unfinished.
Controller RX snapshots capture two distinct exact 31-byte Nordic markers,
both with stock DTM identity and with BIS address/CRC/whitening on data channel
0. Silent controls leave the record empty; both devices pass restoration checks.
An additional one-shot Pico SDK probe delivers each of two distinct 26-byte
received markers as native HCI ISO, using a 250-ms host-assisted task wake.
The sibling `rp2040_audio --features rx-diagnostic` also receives the exact
26-byte marker through Embassy's normal `Runner::run` and `BtDriver`;
its silent and bad-CRC controls receive none. Captures and reset checks are retained in
`work/embassy-normal-rx-series/`.

A separate stream blob removes host-assisted wake: radio callbacks wake the
stock transport task directly. With `--features rx-stream`, Embassy receives
500 of 500 distinct 26-byte SDUs with matching payload indices and HCI
sequences, sending no HCI commands during the receive window; silent and
CRC-mismatch controls receive none. Evidence:
`work/embassy-normal-rx-stream-series/`. Export it with
`--bis --iso --stream`. These establish repeated fixed-channel reception and
HCI egress, not BIS synchronization, hopping or encryption.
See [`analysis/controller-rx.md`](analysis/controller-rx.md).
Export the separate one-shot RX diagnostic (not a synchronized BIS receiver):

```sh
python3 analysis/check-controller-rx.py --bis --iso --output-blob work/iso-blob-rx
```

This emits a 1024-byte patch, matching 112-byte allocator and ABI manifest.
The CRC-mismatch control observes 500 error-branch packets and zero ISO deliveries.
Embassy's installer and production runner now pass live tests on RP2040 Pico W;
RP2350 remains compile-only. Transport fixes and their exact evidence are in
[`analysis/embassy-transport.md`](analysis/embassy-transport.md).

Current Rust tool parses/rebuilds Pico SDIO firmware records, extracts sparse
controller regions, verifies hardware dumps, and converts Broadcom HCD patches
to Pico SDIO firmware format.

`cyw43_iso_re::iso` parses and encodes HCI ISO packets with direction-aware
validation of handles, timestamps, fragment boundaries, lengths and packet
status. `iso_sdu` adds bounded per-handle SDU fragmentation/reassembly, preserves
timestamps/status, and reports sequence discontinuities and abandoned partial
SDUs. `iso_credit` accounts for the dedicated controller ISO transmit pool
reported by LE Read Buffer Size V2, including per-handle completion and teardown.
`isoal` implements unframed segmentation/recombination for one SDU per ISO
interval, with fixed burst slots, padding, and explicit loss. `isoal_framed`
adds timestamped aggregation, segmentation across events, reassembly and bounded
loss reporting. Both require a Link Layer to order/de-duplicate PDUs and supply
timing and flush deadlines; neither is deployed in the controller.
`iso_channel` implements allocation-free CSA#2 event/subevent channel selection,
checked against Bluetooth Core sample vectors.
`bis` derives and validates BIS/BIG Control access addresses and CRC initial
values, checked against Core and all 32 Zephyr reference stream identities.
`broadcast` encodes/validates cleartext BIGInfo, ACAD and single-BIS LC3 BASE;
its BASE and announcement wrappers also parse through the pinned Trouble stack.
`broadcast_advertising` builds LE1M primary, auxiliary and periodic discovery
PDUs with SyncInfo and checked offsets. The experimental controller transmitter
uses these PDUs; stock Nordic SDC receives its periodic advertising and BIS.
`iso_control` encodes CIG/CIS/BIG configuration and HCI data-path commands,
validates returned handles/timing, and tracks asynchronous establishment.
Command credits, event masks and advertising/ACL prerequisites remain caller-owned.
`le_audio` parses/encodes LC3 configuration LTVs, calculates channel/frame/SDU
sizes, and checks unicast QoS field bounds and SDU capacity. Optional `lc3`
encodes/decodes PCM using caller-owned working buffers, with multichannel SDU
ordering and packet-loss concealment. Its vendored `lc3-codec` 0.2 backend has
two corrected 8 kHz indexing bugs; see `vendor/lc3-codec/PATCHES.md`. Backend is
not Bluetooth qualified; target audio throughput has not been measured.
`bap` parses PACS capabilities, matches LC3 configurations, encodes all eight
single-ASE ASCS commands, and parses control responses and ASE notifications.
Its single-ASE client waits for both a successful control response and matching
state notification, with explicit recovery after uncertain writes or disconnects.
GATT discovery, security, characteristic I/O, and stream policy remain caller work.
The existing sibling `trouble_audio` project supplies those GATT/profile roles;
this crate's protocol helpers do not replace that stack.
`examples/trouble-composition` pins that stack, compiles actual ASCS discovery /
subscribe / write procedures, and runs its `RawLc3` receive path through this
decoder with sequence-wrap/loss recovery. See its README for nightly commands
and the distinct HCI transport versions. Live GATT procedures remain untested.

Optional `embassy` connects validated ISO packets to Embassy CYW43 0.7 /
`bt-hci` 0.8, enforcing aligned SDIO size and controller credits. Failed or
cancelled writes require reset. Stock CYW43439 cannot initialize this ISO
session because it has no ISO buffers. `examples/embassy` compiles against the
real driver and retains outgoing fragments across credit waits.

The library supports `no_std` with `alloc`; codec working buffers are caller-owned,
while SDU/credit/profile APIs require an allocator. Firmware CLI needs default
`std` feature. No HCI commands or host-side codec can supply missing radio behavior.

```sh
cargo test --all-features --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo check --no-default-features --features lc3,embassy --lib --target thumbv6m-none-eabi
cargo check --manifest-path examples/embassy/Cargo.toml --locked --target thumbv6m-none-eabi
```

`tests/iso_pipeline.rs` checks synthetic data through unframed ISOAL and, with
`lc3`, real encoded audio through credit-limited HCI, framed ISOAL, simulated PDU
loss, HCI reassembly and decoding. Separate codec tests cover all six rates and
both durations, signal quality, multichannel ordering and malformed frames.
Independent Google liblc3 decode vectors are retained in `tests/fixtures/lc3`.
`cargo run --manifest-path examples/lc3-composition/Cargo.toml --locked`
checks that the fixed-buffer backend coexists with upstream's allocating codec.
These checks do not establish CIS/BIS interoperability or over-air audio.

```sh
cargo run -- inspect /path/to/43439A0_btfw.bin
cargo run -- patch-map /path/to/43439A0_btfw.bin
cargo run -- instrument-hook work/sdk0065.bin work/sdk0065_hook_counter.bin 0x00074419
cargo run -- c-header work/sdk0065_hook_counter.bin work/custom-fw/cyw43_btfw_43439.h
cargo run -- verify /path/to/43439A0_btfw.bin
cargo run -- extract /path/to/43439A0_btfw.bin work/43439
cargo run -- decode-dump capture.txt work/cyw43439-btrom.bin
cargo run -- import-hcd patch.hcd output.bin VERSION_STRING
```

Build Pico W probes:

```sh
PICO_SDK_PATH=/var/home/sawyer/projects/pico-sdk \
  cmake -S hardware -B work/hardware-sdk22 -G 'Unix Makefiles'
cmake --build work/hardware-sdk22 -j4
```

Targets:

- `picow_btrom_dump`: repeated CRC-protected controller ROM dump.
- `picow_btram_dump`: repeated CRC-protected `0x00200000..0x00220000` RAM dump.
- `picow_hci_probe`: capability bitmap plus direct ISO opcode probe.
- `picow_hook_counter`: advertises and reports entries into an instrumented hook.
- `picow_config_probe`: captures configuration-validation RAM and ready status.
- `picow_runtime_probe`: verifies temporary controller code execution through a post-boot HCI callback.

Hardware result: stock `.0065` exposes no ISO features, commands, data path, or
hidden handlers. Newer `.0071` 1YN patch does not boot on Pico W configuration.
Infineon lists CYW43439 LE Audio and LE Isochronous Channels as unsupported.
See `analysis/hardware-results.md` and `analysis/scheduler-hooks.md`.

Rust can reproduce small Patchram hooks, but full controller ROM replacement is
not possible through Patchram. Stock firmware requires an external ISO-capable
controller for LE Audio. The experimental RAM patches now transmit a constrained
broadcast that stock Nordic SDC receives; they are not a general controller
replacement. Embassy host changes alone cannot create missing radio behavior.

`instrument-hook` appends a 48-byte wrapper after the selected hook's Patchram
code segment. It increments a private 32-bit counter, captures entry registers
`r0` through `r3`, then tail-branches to the unchanged stock hook. The command
prints wrapper and telemetry addresses for a backplane RAM read. This is the
first scheduler-hook experiment; instrumentation adds execution time. The
redirected image invalidates the configuration's ECDSA P-256 signature and fails
controller initialization with error `180`, so wrapper execution and lifecycle
counts remain unvalidated. See `analysis/config-validation.md` for ROM evidence,
live controls, and `python3 analysis/check-config-validation.py`. The probe exits
on startup failure and reads telemetry only after HCI reaches working state.
See the controlled comparisons in `analysis/hardware-results.md`.

Post-boot instrumentation now executes successfully without changing the signed
configuration: `picow_runtime_probe` temporarily wraps a RAM HCI callback,
checks execution with a version query, and restores it. The optional state-7
probe observes the scheduler hook during advertising and a controlled BLE
connection. This is instrumentation progress, not ISO support. Details and
limits are in `analysis/scheduler-hooks.md` and `analysis/iso-event-path.md`.

Build the runtime experiment (use `-DSTATE7_PROBE=OFF` for the command-only check):

```sh
python3 analysis/check-runtime-probe.py --prepare
PICO_SDK_PATH=/var/home/sawyer/projects/pico-sdk cmake -S hardware -B work/hardware-runtime \
  -DCYW43_CUSTOM_BTFW_DIR="$PWD/work/runtime-hook-probe/custom-fw" \
  -DSTATE7_PROBE=ON -DSTATE7_CONTROL=OFF
cmake --build work/hardware-runtime --target picow_runtime_probe -j4
```

`STATE7_CONTROL=ON` runs the same advertising/connection experiment with the
original hook prologue. The instrumented probe advertises for 35 seconds and
prints its address for the test peer. It attempts restoration before reporting
its result; uncertain cleanup retains executable bytes for recovery through SWD.

The scheduler phase experiment uses a separate 112-byte reservation and clones
the stock callback table, replacing only its phase callback. Build it separately:

```sh
python3 analysis/check-runtime-probe.py --prepare --phase
PICO_SDK_PATH=/var/home/sawyer/projects/pico-sdk cmake -S hardware -B work/hardware-phase \
  -DCYW43_CUSTOM_BTFW_DIR="$PWD/work/event-phase-probe/custom-fw" \
  -DEVENT_PHASE_PROBE=ON -DSTATE7_CONTROL=OFF
cmake --build work/hardware-phase --target picow_runtime_probe -j4
```

`STATE7_CONTROL=ON` leaves the cloned table's callback unchanged. Both modes
restore the global table pointer but retain the clone and wrapper until controller
reset: existing events keep their own pointers. See `analysis/event-phase-probe.md`
for live results, `analysis/event-ownership.md` for lifetime constraints, and
`analysis/event-timing.md` for clock and accessor contracts.

`EVENT_PHASE_TIMING=ON` uses the same 112-byte reservation to capture a phase-2
stored-clock delta after the stock callback returns. Build commands, controls,
and measurement limits are in `analysis/event-phase-timing.md`.
`analysis/radio-event-contract.md` maps a separate 40-byte scheduler event,
deadline selection, and asynchronous cleanup. Its stock completion changes
controller clocks; it is not an inert event constructor or ISO implementation.

`SRAM_EXEC_PROBE=ON` now allocates a guarded 512-byte resident SRAM block and
executes a position-independent HCI callback from it. Live checks prove one
allocation across two queries, callback execution, and stock restoration.
See `analysis/sram-execution.md` for reproduction and limits.

`CONTROLLER_TIMER_PROBE=ON` uses that allocation for a periodic SRAM callback.
Live checks show callbacks during two windows without HCI polling, successful
cancellation, and stock restoration. See `analysis/controller-timer.md` for
commands and captures. This establishes independent CPU timer execution; it
does not establish radio scheduling or ISO timing.

`CONTROLLER_EVENT_PROBE=ON` now validates one independent scheduler event:
registration returns, completion and asynchronous cleanup each run once, the
object clears, and checked scheduler references detach. Stock restoration
passes. See `analysis/controller-event.md` for reproduction and captures.
This event has no radio payload. `EVENT_PHASE_RADIO=ON` now captures stock
phase-1 radio mode and transmit-header registers during a live BLE connection.
Instrumented and control runs pass with stock restoration; see
`analysis/event-phase-radio.md` and `analysis/check-phase-radio.py`.
`CONTROLLER_TX_PROBE=ON` transmits a complete custom SRAM payload through the
stock DTM event. Independent Nordic reception and software CRC verification
confirm 104 logged custom packets and 129 stock controls. Both devices are
restored; see `analysis/controller-tx.md`. This is verified raw payload
transmission, without BIS timing or standard discovery.
`CONTROLLER_BIS_PROBE=ON` additionally verifies BIS-derived address/CRC and
data-channel whitening through 165 logged complete CRC-valid packets. See
`analysis/controller-bis.md`; this run still uses fixed-channel DTM recurrence.
`CONTROLLER_AUDIO_PROBE=ON` rotates actual LC3 SDUs at measured 10-ms spacing.
`CONTROLLER_HOP_PROBE=ON` adds CSA #2 hopping: 27 CRC-valid received packets,
26 measured intervals of 10000..10001 us, and successful host PCM decoding.
See `analysis/controller-audio.md` for captures, checks and receiver limits.
`CONTROLLER_DISCOVERY_PROBE=ON` adds primary/auxiliary/periodic advertising:
an independent raw receiver follows parsed discovery fields through 53 packets.
The three-primary-channel variant also establishes periodic and BIG sync with
stock Nordic SDC, delivering matching 26-byte HCI ISO SDUs. That first receiver
used a preconfigured identity; the later native variant repeats discovery and
also passes normal scan-derived acquisition.
See `analysis/controller-discovery.md` for evidence and limits.
`CONTROLLER_NATIVE_ISO_PROBE=ON` adds real HCI type-5 ingress, a bounded FIFO,
LE Read Buffer Size V2 and completion credits. A live 20-SDU controller test
drains, underruns and resumes using only returned credits during streaming.
Stock Nordic SDC discovers the broadcast, synchronizes and independently receives
18 matching distinct host-fed SDUs, including resumed audio after the intentional
gap; decoded PCM is retained. Only initial frames 0 and 1 are unobserved in this
run. `CONTROLLER_CONTINUOUS_ISO_PROBE=ON` retains discovery during streaming:
1,000 host SDUs complete with all credits returned, and a late-joining stock
SDC receiver obtains 199 consecutive matching SDUs. See
`analysis/controller-native-iso.md` for captures and limits.
CIS link procedures, multi-subevent scheduling and normal controller lifecycle
remain unfinished. Application GATT/profile and codec integration belong in
separate projects.

Build the focused counter probe against that image:

```sh
PICO_SDK_PATH=/var/home/sawyer/projects/pico-sdk cmake -S hardware -B work/hardware-hook \
  -DCYW43_CUSTOM_BTFW_DIR="$PWD/work/custom-fw" \
  -DHOOK_COUNTER_ADDRESS=0x000d0fac
cmake --build work/hardware-hook --target picow_hook_counter -j4
```
