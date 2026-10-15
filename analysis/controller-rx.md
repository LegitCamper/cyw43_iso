# Constrained controller RX snapshot contract

Live result: stock DTM RX places a received 31-byte marker at `0x370a00`,
with raw header `0x1f001f00` at `0x318b98`. Two different markers after reset
and an untouched silent control establish one-shot DTM payload reception.
Fixed-channel reception with custom BIS identity and whitening also passed
the controls below. Continuous delivery and synchronized BIS remain separate
experiments.

The initial mapping used no hardware. Source is clean ROM
`work/cyw43439-btrom-clean.bin`, SHA-256
`d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038`.
GNU Arm objdump on `work/btrom-clean-patchram.elf` supplied instruction evidence.
REA's Ghidra provider remains unconfigured (`GHIDRA_INSTALL_DIR` absent);
these findings are ordinary local static evidence, not REA Evidence IDs.

Run `python3 analysis/check-controller-rx.py` to check ROM identity, callback
table, literals, instruction anchors and the position-independent assembly
layout. This command alone supplies no live RX evidence.

## Prepared resident wrapper

`hardware/controller_rx.S` is a 512-byte position-independent wrapper, prepared
for the existing allocator/table installation path. Build
and emulate it with:

```sh
uv run --with unicorn python analysis/check-controller-rx.py --emulate --output work/controller-rx.bin
```

The checker links at `0x209bac` and `0x217e78` and requires identical bytes.
The allocation must be word aligned; the wrapper preserves incoming eight-byte
stack alignment. Neither link address grants ownership. Host must use its actual guarded allocation,
patch cloned table `allocation + 256 + 0x18` to `allocation | 1`, verify the
entire blob and original table pointer, then publish the cloned table through
`0x203a80` before DTM start. No installation, allocation or cleanup code is
embedded in this wrapper.

| Allocation offset | Size | Field |
| --- | --- | --- |
| `0` | at most 256 | RX hook and literals |
| `256` | 44 | Exact stock DTM callback table; host patches only `+0x18` |
| `316` | 4 | Leading canary `0x52585331` |
| `320` | 4 | State: 0 empty, 1 claimed, 2 complete |
| `324` | 4 | Version 1 |
| `328` | 4 | Raw IPSR |
| `332` | 4 | Event pointer |
| `336` | 4 | Parameter pointer |
| `340` | 4 | Raw status |
| `344` | 4 | Raw header |
| `348` | 6 | Before counters, three little-endian halfwords |
| `354` | 6 | After counters, same order |
| `360` | 4 | Sign-extended RSSI |
| `364` | 4 | RX mode |
| `368` | 32 | Raw candidate payload window |
| `400` | 4 | Trailing canary `0x52585331` |

The wrapper reads status exactly once before eligibility testing and captures
only the first matching callback with status bit 2 set. Other status values
forward directly to stock without claiming or modifying the record. The saved
status value supplies both the eligibility decision and the record.
One-shot claim masks interrupts only around the state test/store, restoring
incoming PRIMASK before copying or stock callback work. A `dmb` precedes
publishing state 2. Invalid event/parameter/mode guards and states 1/2 tail-call
stock directly. The successful path invokes stock exactly once and preserves
its caller-register outputs and flags as well as the incoming callee-saved
registers. No radio registers are written by the wrapper.

Emulation executes actual assembled hook plus the actual ROM `0x7143e`
counter/rearm body. RSSI helpers, finite-test bookkeeping, delay, hardware
helper and scheduler removal are stubbed. Tests compare direct stock execution
against wrapped execution for status values 0 through 7, counter wrap,
PRIMASK 0/1, guard rejection, claimed/completed records, pending removal and
IPSR 16. They check stock invocation count, eight exact window reads, canaries,
commit ordering, stack alignment, register/flag outputs, event/parameter
updates and identical MMIO writes. Non-bit-2 callbacks leave the record untouched.
The checker also requires one wrapper status read plus stock's own read.
Emulation proves neither live helper
behavior nor timing, read side effects, interrupt nesting or RF reception.

The first live silent control, using the older unconditional snapshot wrapper,
is retained in `work/controller-rx-snapshot-series/silent/tx-instrumented.txt`.
It recorded state 2, status `0x00100041`, header 0, and counters changing from
`0,0,0` to `0,1,0`, with no transmitter. This is evidence that stock DTM calls
receive processing without a usable packet. Its window bytes must not be
treated as received payload. The status-bit-2 eligibility gate addresses that
observation; the revised wrapper passed the fresh controls below.

## Live DTM snapshot validation

Artifacts are in `work/controller-rx-bit2-snapshot-series/`, with separate
`silent/`, `marker/`, and `alternate/` source snapshots, manifests, ELF files,
captures and restoration logs. All three manifests pass; generated headers
and the unique embedded ELF blob match the checked assembly. Blob SHA-256:
`a05b349441c3a45e66d9e4c08ed29cb540f79052017f47ff54ed46fda52eca15`.
All three Pico ELF files have SHA-256
`373b7ba63ee12fc3bce1e416377c1bdf06eb7e8bed1398198a8999de89755144`.

| Control | State | Status | Raw header | Before / after counters | RSSI | IPSR |
| --- | --- | --- | --- | --- | --- | --- |
| Silent | 0 | Untouched | Untouched | Untouched | Untouched | Untouched |
| Marker `00..1e` | 2 | `0x00110036` | `0x1f001f00` | `0,0,0` / `1,0,0` | -60 | 26 |
| Alternate `80..9e` | 2 | `0x00110036` | `0x1f001f00` | `0,0,0` / `1,0,0` | -59 | 26 |

Both positive records exactly match their 31-byte marker. The 32nd captured
byte was `d5` and `8c`, respectively, and is unspecified padding, not packet
payload. Independent Nordic TX used BLE 1M at 2402 MHz, access address
`0x71764129`, CRC init `0x555555`, whitening disabled and on-air header bytes
`00 1f`. That on-air 16-bit header is `0x1f00`; the observed 32-bit controller
header is `0x1f001f00`. No semantics are assigned to its duplicated upper half.
Both transmitter logs report `sent=500 disabled=true`.

Each capture passes the ordered lifecycle checks and two equal raw record
reads. All three stock Pico restoration logs report absent controller ISO
support and LMP subversion `0x2310` twice. Final Nordic RRAM (1,560,576 bytes)
and UICR (4096 bytes) readbacks match the original backup byte for byte.
This proves one snapshot per run, not sustained RX, BIS anchor/channel
tracking, ISOAL delivery or coexistence.

## Bounded fixed-channel BIS-format variant

`check-controller-rx.py --bis --emulate` builds a 1024-byte variant with the
same RX hook, callback table and guarded record offsets. A phase wrapper at
allocation `+416` replaces table `+0x10`; configuration trace at `+768` holds
five words: completed setup count, control, access address, CRC init and
logical whitening channel.

Live artifacts in `work/controller-rx-bis-prepared-series/` pass independent
manifest, source/header/unique ELF blob, capture lifecycle and restoration
checks. Common Pico ELF SHA-256:
`32a015b5ea8ff924f7909fb16a2f8b8404f864c5c25bcee649c7214f1210ae0b`.
Checked 1024-byte blob SHA-256:
`2acaf250016cab6a6d523fddd5fb816a31aa8c7bde6f8c8196e4871a494f8708`.

| Control | State | Status | Raw header | Before / after counters | RSSI | IPSR |
| --- | --- | --- | --- | --- | --- | --- |
| Silent | 0 | Untouched | Untouched | Untouched | Untouched | Untouched |
| Marker `00..1e` | 2 | `0x00110036` | `0x1f001f00` | `0,0,0` / `1,0,0` | -69 | 26 |
| Alternate `80..9e` | 2 | `0x00110036` | `0x1f001f00` | `0,0,0` / `1,0,0` | -58 | 26 |

All three final phase records are stable, with count 1, control
`0x01302340`, access address `0x718ebed6`, CRC init `0x123401` and logical
channel 0. Nordic TX logs report BLE 1M at 2404 MHz, matching identity,
whitening enabled and 500 transmissions followed by disabled radio. Both
31-byte markers match exactly; final raw-window bytes `56` and `0f` are
unspecified padding. Each Pico stock restore passes twice; final Nordic
RRAM and UICR readbacks match original backups byte for byte.

The earlier `work/controller-rx-bis-snapshot-series/` attempt omitted the
custom-firmware build directory and failed its scratch preflight. It supplies
no custom reception evidence; its Pico stock restore passed. The prepared
series above contains the corrected build and successful controls.

ROM `0x71546` dispatches phase 0 to setup `0x71252`. That setup overwrites
`0x318b6c`, `0x318b70`, `0x318ba4` and RF selector `0x3186a4`, then enables
RX before returning. The wrapper must therefore apply custom identity after
stock on every matching phase 0, not only once. It writes access address
`0x718ebed6`, CRC init `0x123401`, ORs control mask `0x300000`, and writes
logical data channel 0 to `0x318bf4`. It retains stock rearm and removal.

The HCI DTM channel is 1, selecting 2404 MHz, while parameter byte `+7`
must be **2**: RX command handler `0x52424..0x52428` doubles the HCI channel
before storing this RF offset. Stock setup consequently writes `0x82` to
`0x3186a4`. Guards require phase 0, the original singleton and parameter
identities, RX mode 1 and RF offset 2.

Actual ROM phase setup executes in the emulator with external hardware
helpers stubbed. Checks cover guard rejection, stock invocation once,
caller outputs/flags, stack alignment, unchanged PRIMASK, exact ordered
post-stock configuration writes, repeated configuration and count wrap.
RX callback emulation separately verifies that access address, CRC init,
control, whitening channel and RF selector survive every status branch
and pending removal. These checks do not prove unmodeled helper effects.
The phase wrapper also leaves a brief initial stock-identity RX window
between stock RX enable and its identity writes; start peer transmission
only after the host's ready marker and validated phase readback.

These controls establish custom packet reception on one fixed channel.
A mismatched-access-address control was not run. Synchronized BIS acquisition,
channel tracking, continuous RX and host ISO delivery remain separate
milestones.

The task-delivery fixture uses 26 nonrepeating bytes `00..19` or
`80..99`, preserving the existing 31-byte controls. The same eight-word
snapshot remains bounded; its last six bytes are outside that packet.
The checker accepts exactly 26 or 31 expected bytes and compares only that
explicit length. Raw header `0x1a001a00` was measured in both 26-byte
positive controls below. Emit from task context after snapshot
completion, never from the radio interrupt; one snapshot must become at
most one complete HCI ISO SDU. Continuous buffering and loss policy remain
outside this one-shot contract.

`--bis --iso --emulate` now builds a complete 1024-byte image with a task
bridge at `+576`, delivery state at `+736` (sent flag, last emitter result),
unchanged BIS trace at `+768`, and ISO emitter at `+800`. Assembly source is
`hardware/iso_rx_task.S`; its 336-byte position-independent overlay is checked
by `analysis/check-iso-rx-task.py`. The host must verify and own the existing
transport callback `0x44105` at context `+0x100`, install the bridge only
after all image/state writes, then restore that owned callback before reset.

The bridge calls stock first, then accepts only task context, event bit 0,
completed snapshot, exact 26-byte expected raw header, mode 1 and status
bit 2. It emits sequence 0 on handle `0x42`, preserving stock outputs/flags.
The current bridge additionally requires the bit-2 counter to advance by one
modulo 65536 and the other two counters to stay unchanged. Its emulator checks
normal increment, wrap and each mismatch. This tests snapshot consistency;
it does not independently establish CRC-status semantics.
Ring-full leaves the snapshot available for another task wake; success sets
the sent flag and suppresses duplicates. Emulation runs the real emitter
with stock transport/ring/notification calls stubbed and checks guards,
both allocation addresses, PRIMASK preservation, retry then success,
duplicate suppression and unchanged snapshot/BIS trace. The emitter's own
checker separately executes the ROM writer. Initial task wakes come from
host HCI queries in the live test below.

## Live one-shot controller-to-host ISO delivery

`work/controller-rx-iso-series/` contains a silent control and two independent
26-byte markers. All archived manifests pass; rebuilding the archived RX,
task and emitter assembly reproduces the unique 1024-byte blob embedded in
each ELF. Pico ELF SHA-256:
`c90015afde624bef98b07e3d2f2fdf634601b7bda00b24122e559a3695e03ba0`.
Live blob SHA-256:
`0853cd270e4072d86d4b575977aabeab1c3c1bb28914ffe3b40f8314927f0a74`.

| Control | Snapshot | Raw header | Status | Counter change | RSSI | Inbound ISO |
| --- | --- | --- | --- | --- | --- | --- |
| Silent | State 0, untouched | Untouched | Untouched | Untouched | Untouched | None |
| `00..19` | State 2 | `0x1a001a00` | `0x00110036` | `0,0,0` to `1,0,0` | -65 | Exactly one matching 26-byte SDU |
| `80..99` | State 2 | `0x1a001a00` | `0x00110036` | `0,0,0` to `1,0,0` | -64 | Exactly one matching 26-byte SDU |

Both positive snapshots have IPSR 26. Only the first 26 bytes of their
32-byte windows are payload. Nordic sources use BLE 1M at 2404 MHz,
access address `0x718ebed6`, CRC init `0x123401`, whitening enabled and
header `0x1a00`; each reports 500 transmissions and disabled radio afterward.
Each host capture contains one H4 type-5 packet, 35 bytes including H4 type,
with header `05 42 20 1e 00 00 00 1a 00` and its exact marker. This means
handle `0x42`, complete SDU, data total length 30, sequence 0 and SDU length
26. It is observed inbound traffic before ordinary host ISO support checks.

Positive task records report delivered 1, result 0, count 1, errors 0 and
stable reads; silent reports delivered 0 and count 0. The strict checker
requires successful installation, poll-assisted marker, task wakes, exact
packet count/header/sequence/payload, task telemetry and owned transport
callback restoration before global DTM table restoration. Removing those
ISO markers from a historical BIS-only capture fails ISO validation.

All three Pico stock restoration logs pass twice. Final Nordic RRAM
(1,560,576 bytes) and UICR (4096 bytes) independently match the original
backup byte for byte. Task wake requests occur every 250 ms in this host
harness; there is no autonomous RX delivery or latency guarantee.

These live runs predate the additional counter-consistency guard. Current
guarded blob SHA-256 is
`8dfcf7eee1ad5b54aec1022794f183442f146a58ad56359890ecdcf66b13f610`;
its normal/wrapped increments and mismatch rejection pass emulation, and
the separate live CRC controls below use this revision. The old live records
already satisfy those counter comparisons. Continuous delivery, autonomous
task wake, BIS synchronization and Embassy host reception remain separate
milestones.

## CRC-init mismatch control

`work/controller-rx-iso-crc-series/` uses the guarded blob above. All three
manifests, checked image/header and unique ELF embeds pass independently.
Common Pico ELF SHA-256:
`b2ff2e045bfe0bf506c3d91082588ebde1593762d98cd43c77b041a012fad318`.

| Control | Active counters: bit2, neither, bit1 | Snapshot | ISO delivery |
| --- | --- | --- | --- |
| Silent | `0,1,0` | Untouched state 0 | None; clean task/lifecycle checks |
| Matching CRC init `0x123401` | `492,0,7` | Exact `00..19`, raw header `0x1a001a00`, RSSI -66 | One exact complete SDU |
| CRC init `0x123400` | `0,0,500` | Untouched state 0 | None; clean task/lifecycle checks |

The negative transmitter toggles only one CRC-init bit, retaining access
address, whitening, frequency, header and 26-byte marker. Both transmitter
logs report 500 packets and disabled radio afterward. Active counters are
read before stopping DTM. They are observations, not a packet-loss or RF
timing measurement. In the matching case, the captured callback increments
`0,0,0` to `1,0,0`; the additional counter guard permits its one SDU.

`--bis --iso --bad-crc --probe-capture FILE --peer-capture PEER` requires
bounded bit1 activity with no bit2 activity, ordered active-counter evidence,
untouched record, no ISO packet, clean task teardown and the exact independent
transmitter configuration. Positive, silent and negative checks all pass.
Each Pico stock restoration passes twice. Final Nordic RRAM and UICR
readbacks independently match original backups byte for byte.

This isolates CRC-init mismatch rejection for this fixture: 500 wrong-init
transmissions produce bit1 activity and no delivered SDU. It does not assign
universal meanings to every status bit or prove rejection of every malformed
packet, PHY configuration or concurrent radio condition.

## Stock DTM receive lifecycle

Singleton event `0x209788` points at parameters `0x2097b8` through event
`+0x1c`. Parameter byte `+6 == 1` selects RX. Table pointer `0x203a80`
initially contains `0x8b634`; table `+0x18` contains Thumb entry `0x7143f`.

Dispatcher `0x712f2` calls `0x16e96`, then dispatches table `+0x18` for
returned mask `0x1`, and table `+0x1c` for returned mask `0x4`. These are
dispatcher masks, separate from the receive-status bits below. `0x16e96`
acknowledges the pending interrupt at `0x314004`; a snapshot wrapper must
not call it again.

Receive callback `0x7143e` runs this sequence:

1. Call `0x5d9b6(0, -2)` and `0x5db0a(0, -2)`. Copy byte `+0x20` from
   the first returned object into event `+0x2c`. Getter `0x71566` later reads
   that byte signed. This is the stock RSSI value; no calibration accuracy
   follows from the mapping.
2. Store halfword 5 at event `+0x0c`.
3. Read word `0x318bac`. If bit 2 is set, increment parameter halfword
   `+0x0a`; otherwise, if bit 1 is set, increment `+0x0e`; otherwise increment
   `+0x0c`. Each wraps modulo 65536. Bit 2 wins when both bits are set.
4. Pass the original status word to `0x75726`, then call `0x602e(3)`.
   The first helper has finite-test bookkeeping and can request removal.
5. If event word `+0x20` is nonzero, tail-call removal helper `0x713e4`.
6. Otherwise save word `0x318014`. For event byte `+0x0f == 1`, clear its
   bit 7, call `0x1711a(2)`, set bit 25 of `0x31801c`, then restore the saved
   word with bit 7 set. Other values write the saved word unchanged.

Use neutral counter names `bit2`, `neither`, `bit1`. Static branches do not
justify naming every status bit CRC, timeout or malformed payload. In
advertising receive `0x70828`, bit 2 gates sender-address extraction and bit 3
is stored separately. This supports a usable-packet interpretation for bit 2,
but is not an independently measured PHY status definition.

Constructor `0x711fe` clears only parameter counter `+0x0a` locally. Do not
assume `+0x0c/+0x0e` start at zero; record all three before and after the
same callback. Global initialization may clear them elsewhere.

## BLE receive payload evidence

| Code | Observation | Implication |
| --- | --- | --- |
| `0x47f82`, copy at `0x47fe2..0x47fe8` | Decode length from incoming header; conditionally subtract four encrypted-packet bytes; copy from literal `0x370a00` to host packet buffer `+4` | Connected BLE payload source, distinct from TX windows `0x370800/0x370900` |
| `0x7080e` | Copy word and halfword from `0x370a00` into a six-byte address; derive address type from header bit 6 | Advertising payload begins with sender address at this window |
| `0x70830..0x70848` | Read status `0x318bac` and header `0x318b98` | Concrete raw status/header pair used by advertising RX |
| `0x464c2`, copy at `0x464d8..0x464de` | Copy 20 bytes from `0x370a10`; check connection interval, latency, supervision timeout and hop | Connection-request interpretation corroborates BLE ownership |

The connection-request path reads sender address at `0x370a00/+4` and
access address at `0x370a0c`. Its 20-byte copy starts four bytes into LLData,
after the access address, and reaches `0x370a23`. Thus the proposed 32-byte
window is within a range already read by stock BLE code. This is not proof
of DTM-mode stability, retention time, or absence of read side effects in
every radio mode.

Copy routine `0x85fbc` rounds a positive residual length up to whole words.
For a 31-byte snapshot, copy exactly eight aligned words into a 32-byte
destination. Do not call it with an unbounded radio-header length. The
generic header-length helper `0x44532` selects masks from live mode state;
using `header >> 8` as a universal length decoder is unsupported. The first
marker experiment uses an exact expected header provided by its TX setup.

Classic RX helpers around `0x1df94` use `0x370000`; they supply no evidence
for DTM payload placement. Do not widen the experiment to inspect arbitrary
radio windows when the specific BLE candidate can be tested directly.

## DTM experiment procedure

This contract is for the hardware owner; no executable RX hook is installed
by this analysis. Reuse the proven resident allocation and cloned DTM-table
installation approach from [controller-tx.md](controller-tx.md).

1. Start from reset, stock DTM only, no other connections or custom TX owner.
   Allocate resident code/table/record with the existing guarded allocator.
   Copy all 44 stock callback-table bytes, replace only `+0x18`, and preflight
   singleton, parameter pointer, original table, callback entry and RX mode.
   Publish only initialized storage. Retain storage until reset.
2. Use a known nearby transmitter on one fixed channel with stock DTM
   access address/CRC and a nonrepeating 31-byte marker. Record its exact
   header and expected payload before testing. The existing TX marker
   `00 01 ... 1e` is suitable. Run a silent-transmitter control separately.
3. Wrapper forwards every invocation to stock `0x7143f` exactly once, keeping
   ABI, stack alignment, registers and condition flags. Read status
   `0x318bac` once; forward callbacks lacking bit 2 without claiming storage.
   For the first matching bit-2 invocation only, save IPSR, event/parameter
   identities, all three counters, that saved status, raw header `0x318b98`, and exactly
   32 bytes from `0x370a00` **before** stock callback. Claim the one-shot
   record before copying; nested/repeated callbacks cannot overwrite it.
4. After stock returns, save all three counters and signed event RSSI.
   Publish completion last with memory ordering that makes the full record
   visible. Keep canaries around the record. Later invocations only forward;
   no allocation, HCI emission, retry loop, payload parsing or radio writes
   belong in this hook. The stock callback retains all rearm/removal work.
5. Stop TX, end DTM through the existing host control, read the completed
   resident record twice in transfers no larger than 64 bytes, and require
   byte equality plus intact canaries. Reset and verify stock HCI returns.
   Do not free/reuse storage merely because DTM end returned.

The extra MMIO reads and copy may affect timing. Counter agreement checks
whether the saved status predicts the stock callback's result; disagreement
fails the experiment rather than being explained away as packet loss.
One successful snapshot proves one DTM payload observation only. Repeat with
a different marker after reset to exclude stale-window coincidence. No claim
of continuous reception, BIS channel/anchor tracking, ISOAL delivery or
radio coexistence follows.

## Optional capture check

The C host harness uses the guarded resident allocator and generated checked
header; it does not embed a second hand-maintained image. Build from repository
root with the validated zero-reservation boot header already prepared:

```sh
PICO_SDK_PATH="$PWD/../pico-sdk" cmake -S hardware -B work/hardware-rx \
  -DCYW43_CUSTOM_BTFW_DIR="$PWD/work/sram-exec-probe/custom-fw" \
  -DCONTROLLER_RX_PROBE=ON -DSTATE7_PROBE=OFF -DSTATE7_CONTROL=OFF
PICO_SDK_PATH="$PWD/../pico-sdk" cmake --build work/hardware-rx \
  --target picow_runtime_probe -j4
```

`RX_PROBE ready` follows active-event table, parameter, mode and channel checks.
The receive window lasts 20 seconds. Afterwards the harness ends DTM, verifies
event removal, reads the 88-byte guarded record twice in 64/24-byte chunks,
prints both raw records, restores the owned global table and checks stock HCI.
`RX_PROBE lifecycle=pass` reports those host checks; it does not mean a valid
payload was received. The external runner must still restore stock Pico firmware
and verify any changed Nordic device against its original backup.

`check-controller-rx.py --capture record.json --expected-header 0x1f001f00
--expected-payload 000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e`
checks a completed positive marker record against the live DTM fixture.

JSON fields are `version` (1), `complete` (true), `event` (integer),
`parameters` (integer), `mode` (1), `ipsr` (raw integer), `status` and
`header` (raw 32-bit integers), `before` and `after` (three halfword integers
ordered `+0x0a,+0x0c,+0x0e`), `rssi` (signed byte), `payload` (32-byte hex),
and `canaries` (two integers, both `0x52585331`). Host must separately retain
the two equal raw reads, silent control, TX configuration, stock restoration
and artifact hashes. A JSON conversion cannot prove these lifecycle steps.

For the host probe's `RX_PROBE record=...` line, use `--probe-capture FILE`
instead of `--capture`. This decodes exactly 88 raw bytes from allocation
`+316..+403`, including both canaries. Positive checks still require the
expected header and 26- or 31-byte marker. `--probe-capture FILE --silent` checks
an untouched state-0/version-1 record for the silent control. Repeated printed
records do not count ordinary stock callbacks; an untouched record means no
eligible bit-2 candidate was captured. Printed
records must be exactly two equal records. The checker requires one ready
marker with bounded allocation, then successful stop/event-clear, both raw
records, stable-snapshot with matching completion state, global-table restore,
stock-version response, retained-storage marker and lifecycle pass, in that
order. Missing, duplicate, conflicting and reordered markers fail. Broader
stock capability checks and the subsequent hardware reset remain separate.

`--output-header PATH` exports the validated assembly as
`static const uint8_t controller_rx[512]`, suitable for a generated build
header. Consumers must include `<stdint.h>`. Default checks and header export
need Python and GNU Arm binutils; only `--emulate` requires Unicorn.

## Repeated fixed-channel RX through Embassy

`work/embassy-normal-rx-stream-series/` exercises the separate stream blob
through Embassy's production `Runner::run` and public `BtDriver`, with no HCI
commands during the receive window. Blob SHA-256
`e8ba26ee19b006d48af04cee2b5183782d508d579fd43d363b05925b03b7451f`
(1024 bytes, same 112-byte allocator
`4217cca9174a3d829861c58dab5343f9642f41d1b84e1494ebe526f51bfb8a58`).
Common RP2040 ELF SHA-256
`4e52daa158e94948e6e94fb950cdd2fe7d0dd92278e48196b4cb96f44145cd43`,
uniquely embedding that blob in each of the three archived cases.

| Control | Nordic CRC init | Delivered SDUs | Result |
| --- | --- | --- | --- |
| Silent | No transmitter | 0 | Clean command/stop/version/quiet checks |
| Counter marker | `0x123401` | 500 of 500 | Every payload index and HCI sequence exact |
| CRC mismatch | `0x123400` | 0 | Clean command/stop/version/quiet checks |

The positive transmitter sends 500 distinct 26-byte payloads: little-endian
packet index 0..499 in bytes 0..1, constant `02..19` afterwards. Host-side
records show HCI sequence equal to payload index for all 500 packets, with no
missing, duplicate or reordered delivery. Logged host arrival spacing is
19.9..20.13 ms across the 500 packets. That spacing reflects the transmitter's
approximate DWT-based cadence plus RTT logging, not a controller-measured ISO
interval or a latency guarantee.

Radio callbacks now wake the stock transport task through `0x9061` on the
existing transport context, so delivery no longer needs host command polling.
The single payload mailbox stays owned until the HCI copy succeeds; a busy
mailbox drops newer packets and a full transport ring waits for a later radio
callback. Emulation covers interrupt arrival during copy, ring-full retry,
ring success, count publication and rearm, including accepted-sequence wrap.

Sequence numbers count accepted SDUs; they are not derived from synchronized
BIG events, and the blob performs no loss accounting. Each case was flashed
only after the receiver logged `EMBASSY_RX ready ... mode=stream`. All three
Pico stock restorations pass two capability checks, and both Nordic RRAM
(1,560,576 bytes) and UICR (4096 bytes) readbacks match the original backups
byte for byte. This establishes repeated fixed-channel reception with
autonomous controller wake, not BIG synchronization, channel hopping,
encryption, coexistence or LE Audio profiles.

## Open-loop CSA#2 hopping through Embassy

`--hop` (with `--bis --iso --stream`) builds a separate 2048-byte image,
`18d2465e09e8b158ad73781ede07c2991dd62e2e7823770d83cd446292e6fbf1`, with its
own 112-byte allocator
`dc1c2dfabb13f4a8de26ab8328de4def56944d2d646bbe65cf201295bdbc4d47`. The
validated stream image and its allocator rebuild byte-identical. Allocators
encode their reservation, so the images must never be paired across sizes.

Event zero is programmed from the BIS phase hook. Every matching RX callback,
including CRC failures, runs stock first, lets stock rearm the radio, then
advances a 16-bit event counter once, derives the next CSA#2 channel from
channel identifier `0xcf58` over all 37 data channels, and writes whitening
and the RF selector. Retuning after stock is required: stock rearms with the
old channel. Advancing on CRC failures matters too, otherwise a single
corrupted packet desynchronises the counter permanently. Static checks cover
the ordering, the event-zero channel, 16-bit wrap, both CRC paths, guards and
ring-full interleavings, and 262144 cases agree with the reference.

The nRF54L15 `marker-hop` transmitter walks the same sequence;
`marker-hop-offset` shifts the event counter by one, which CSA#2 maps from
channel 1 to channel 28, and is the negative control.

| Control | Transmitter | Delivered SDUs | Longest aligned run |
| --- | --- | --- | --- |
| Silent | None | 0 | 0 |
| Aligned, abort on first gap | `marker-hop` | 11 | 11 |
| Aligned, full window | `marker-hop` | 15 | 3 |
| Offset by one event | `marker-hop-offset` | 16 | 1 |

The abort-on-gap run received events 0..10 consecutively across nine distinct
CSA#2 channels (1, 5, 7, 8, 16, 25, 28, 29, 30), which a fixed-channel
receiver cannot produce. Retuning therefore works.

Staying synchronised does not. The counter advances only inside the receive
callback, so a missed event parks the receiver on a channel the transmitter
has already left, and reception resumes only where the two sequences coincide
again. Over a full window the aligned and offset transmitters delivered 15 and
16 SDUs, so delivered totals cannot distinguish them; only the longest run of
consecutive transmitter event indices can, at 3 against 1. Host checks are
expressed that way: `--min-run` for the aligned case, and no run at all
permitted for the offset control.

This is open loop. There is no BIGInfo anchor, no timer-driven counter and no
payload-index resync, so none of this is BIG synchronization. All three
archived Pico runs restored stock and passed two capability checks, and the
Nordic RRAM (1,560,576 bytes) and UICR (4096 bytes) readbacks match the
original backups byte for byte. Evidence:
`work/embassy-normal-rx-hop-series/` (abort-on-gap) and
`work/embassy-normal-rx-hop-series-v2/` (full window plus both controls).
