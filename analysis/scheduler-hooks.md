# CYW43439 Patchram hook map

The stock `.0065` configuration contains 18 compact ROM replacement records.
`cargo run -- patch-map work/sdk0065.bin` now recovers them without hard-coded
offsets.

## Record format

Each eight-byte record is:

| Bytes | Meaning |
| --- | --- |
| `0..2` | Patchram pointer bits `31..16`, little-endian |
| `2..5` | 24-bit Thumb ROM pointer, little-endian |
| `5` | Vendor hook kind (`1` or `2` observed) |
| `6..8` | Patchram pointer bits `15..0`, little-endian |

The parser accepts only contiguous records whose odd Patchram pointer resolves
inside a loaded controller-code segment. The live ROM capture validates the
mapping: address `0x00074418` contains `b.w 0x000d060c` after firmware load.

## Stock `.0065` hooks

| ROM Thumb pointer | Patchram Thumb pointer | Kind | Current classification |
| --- | --- | ---: | --- |
| `0x00072d79` | `0x000d03dd` | 1 | LE connection cleanup/control |
| `0x0004b58d` | `0x000d0509` | 1 | Link-state predicate |
| `0x000443d5` | `0x000d0211` | 2 | Veneer into patched routine |
| `0x0004652d` | `0x000d0219` | 2 | Bounds/connection-state fix |
| `0x0004a2ed` | `0x000d0235` | 2 | Command/event index bounds fix |
| `0x00074419` | `0x000d060d` | 1 | **Confirmed state-7 LE timing initialization; also reached by advertising** |
| `0x000682f1` | `0x000d0679` | 1 | LE encryption/control state machine |
| `0x00069995` | `0x000d0249` | 2 | Control-state dispatch veneer |
| `0x00036e6d` | `0x000d08f3` | 1 | RX packet metadata/dispatch path |
| `0x000375e5` | `0x000d0269` | 2 | Descriptor dispatch veneer |
| `0x00025b11` | `0x000d0285` | 2 | Descriptor dispatch veneer |
| `0x0000e7b9` | `0x000d0c2d` | 1 | Baseband/control helper |
| `0x00012071` | `0x000d0c79` | 1 | Baseband/control helper |
| `0x00037fd5` | `0x000d0299` | 2 | Descriptor dispatch veneer |
| `0x00051919` | `0x000d0ebd` | 1 | Radio register-limit fix |
| `0x0000b4b1` | `0x000d02a5` | 2 | Baseband dispatch veneer |
| `0x0005fcb5` | `0x000d02ad` | 2 | Baseband dispatch veneer |
| `0x00002fb9` | `0x000d02b9` | 2 | Baseband dispatch veneer |

`0x000d060c` initializes a block at object offset `0x154` and tail-calls ROM
`0x00074360`. The clean ROM dispatcher at `0x0004ce02(state, object)` invokes
the patched hook only when `state == 7`, then writes `1` to `object + 0x154`.
The clean image is `work/cyw43439-btrom-clean.bin`, SHA-256
`d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038`.

Observed block writes are:

| Object offset | Block offset | Source |
| ---: | ---: | --- |
| `0x155` | `0x01` | OR with flag `0x02` |
| `0x160`, `0x180` | `0x0c`, `0x2c` | halfword at `0x00209848 + 0x34` |
| `0x162`, `0x182` | `0x0e`, `0x2e` | max of halfwords at `+0x36` and `+0x38`; alternate `+0x3a` path exists |
| `0x164`, `0x184` | `0x10`, `0x30` | halfword at `0x00209848 + 0x3c` |
| `0x166`, `0x186` | `0x12`, `0x32` | halfword at `0x00209848 + 0x3e` |
| `0x168`, `0x16c` | `0x14`, `0x18` | halfword at `0x00203ae2` |
| `0x16a`, `0x16e` | `0x16`, `0x1a` | halfword at `0x00203ae6` |

The alternate `+0x3a` selection calls ROM `0x0004ddfe`. Clean ROM confirms this
is `movs r0, #0; bx lr`, so the branch is inactive in the stock image.

The original continuation at `0x00074360` derives these timing fields:

| Object offset | Clean-ROM expression |
| ---: | --- |
| `0x170` | `min(+0x160, +0x16c)` |
| `0x174` | `min(+0x164, +0x168)` |
| `0x17c` | `min(+0x166, +0x16a)` |
| `0x178` | `min(+0x162, +0x16e)` |
| `0x17e` | `max(+0x17c, 2704)` |
| `0x176` | `+0x17c` because `0x0004ddfe` returns zero |
| `0x172` | `+0x178` because `0x0004ddfe` returns zero |
| `0x17a` | bounded form of `(+0x06 * 1250) - (selected_value + 300)`, with minimum `2704` |
| `0x18c` | halfword written by `0x000495dc(object)`; this helper does not return the budget |

`object + 0x06` supplies an interval. Multiplication by `1250`, plus
the constants `2704`, `976`, `300`, and a separate `value * 64` path, strongly
identify microsecond-scale scheduler bounds rather than generic link metadata.
The live advertising trigger below prevents treating every such object as an
established connection solely from this shared timing routine.
`0x000495dc` derives `+0x18c` from two directional timing calculations through
`0x00044adc`, `0x00047bb2`, `0x00047b5e`, and `0x00047b2c`.

### Directional airtime and the `+0x18c` consumer

Focused clean-ROM disassembly and Ghidra output are retained in
`work/ghidra/inspect_clean_helpers.txt`, `work/ghidra/force_helpers2.txt`,
`work/ghidra/inspect_clean_18c.txt`, and
`work/ghidra/inspect_clean_event_programming.txt`. These are local static-analysis
artifacts, not runtime traces or REA Evidence records.

`0x00047b56` unconditionally returns `80` and writes shift count `3` through
its third argument. Consequently, `0x00047b5e(connection, direction, length)`
returns zero for a null connection; otherwise it computes:

```text
adjusted_length = length
if length != 0 and direction in {0, 1}:
    context = lookup_0x47b34(halfword(connection + 0x38))
    if byte(context + 0xd8) & (1 << direction):
        adjusted_length += 4
airtime = 80 + (adjusted_length << 3)
```

The apparent fourth parameter in the initial decompilation is an artifact of
`push {r3,...}` providing a stack slot; `0x00047b56` overwrites that slot before
use. The `80 + 8 * length` expression and conditional four-byte surcharge match
LE 1M packet airtime and a MIC, respectively. The arithmetic is proven; the
TX/RX direction labels and encryption-flag semantics remain inferred.

`0x00047b2c()` returns `halfword(0x00202f9c + 0x16) + 152`.
`0x00047bb2(connection, length0, length1)` adds that global overhead to the two
directional airtimes. `0x000495dc` obtains both lengths through `0x00044adc`,
truncates each to 16 bits, then stores the low 16 bits of the combined result
at `connection + 0x18c` between calls to `0x000045ee` and `0x000045f6`.

`0x000744b6` copies 16 bytes from `connection + 0x170` when the connection is
non-null and `byte(0x00207368) & 0x20` is set. Otherwise it writes only eight
bytes: two copies of the halfword pair at `0x00203ae2` and `0x00203ae6`.
Callers must not treat the fallback as a fully initialized 16-byte tuple.
`0x00044adc` uses the first pair for direction 0 and the second for direction 1,
bounding length by `(time - overhead) >> 3`, with overhead `80` or `112`
selected by connection flag `+0x3c & 4`. Direction 0 additionally applies the
global minimum at `0x00203ae2`.

The proven consumer chain is:

| Address | Observation |
| --- | --- |
| `0x00049618` | Returns `halfword(connection + 0x18c)`; null connection uses `halfword(0x00202f9c + 0x14)` |
| `0x00044b26`, call at `0x00044b4c` | Loads connection from `event + 0x50`, obtains its budget, and compares it with a window calculated using `625`-unit slot conversion and clock corrections |
| `0x00044bd8`, `0x00044c60` | Compare that window with the budget, then with budget plus a global byte margin; update event counters at `+0x118`/`+0x119` and conditionally call `0x00049a8e(0x13, connection_id)` |
| `0x00044f4c` | Uses `0x00044b26`'s decision to set/clear controller register `0x0031801c` bit `0x4000`; calls `0x00044ed2` with selectors 0 and, conditionally, 1 |
| `0x00045318` | Writes the resolved event context's `+0xd8` flags and recomputes its connection's `+0x18c` through the pointer at `+0x50` |

Direct accesses at `0x000484bc` and `0x0004930c` increment **event** `+0x18c`,
while `0x00049204` clears it and `0x00047d96`/`0x00049358` compare it against
event `+0x18e`. These are a distinct count/limit pair, not the connection's
airtime budget. Equal offsets do not imply equal objects.

This establishes a scheduler consumer for the connection budget. The complete
object layouts, all indirect/aliased consumers, and a usable TX descriptor
insertion point remain unresolved. `0x00044ed2` and its callers are candidates
for the next static trace; no controller MMIO probe was needed for this result.

`0x000d08f2` appears to handle received-packet metadata and dispatch. That
classification used the invalid oversized ROM capture and remains provisional.
It is not an established TX descriptor insertion point. The TX event and
descriptor construction path remains unresolved.

## `.0071` comparison

Murata `.0071` has 19 hooks. It adds ROM hook `0x00040579` and shifts most
kind-1 Patchram destinations by `0x18`. The earlier RPI hybrid combined `.0071`
code with the `.0065` configuration, so its hook table still targeted `.0065`
entry offsets. That mismatch is independently sufficient to prevent correct
controller startup; it does not prove board configuration was the only other
problem.

## Hardware probe boundary

Direct backplane reads of controller MMIO (`0x00300000`, `0x00310000`,
`0x00318000`, and `0x00600000`) returned the same unmapped-aperture sentinel
instead of CPU-visible registers. A controller-side vendor `Read_RAM` probe did
not produce output and the Pico/debug-probe USB devices disconnected. Do not
repeat that probe until power is restored and the opcode is isolated against a
known-safe RAM address.

## Next implementation gate

1. Post-boot command, state-7, and scheduler phase wrappers now execute on
   hardware. The signed hook-table modification still fails ECDSA verification;
   runtime instrumentation leaves the signed configuration unchanged.
2. State `7` is now reached during legacy advertising setup before a connection
   completes. Compare instrumented/control lifecycle results below before
   assigning connection-only semantics.
3. Extend the wrapper to capture block values before and after ROM
   `0x00074360`; only fields with observed time behavior become scheduler inputs.
4. Resolve independent LL-event construction, clock selection, and completion
   before insertion. Null callbacks do not isolate radio effects: scheduler
   call `0x0000c9b2` reaches `0x0000a204`, which writes clock selector MMIO
   `0x00318200`. The separate software timer in `analysis/event-ownership.md`
   can test inert periodic execution but does not supply this radio contract.
5. Confirm event timing on hardware, then resolve the actual TX descriptor path
   before attempting one unframed CIS PDU.
6. Only after over-air timing works, add HCI ISO packet `0x05` and ISO commands;
   keep ISOAL and audio queues on RP2040.

## Hook-entry telemetry image

`instrument-hook` emits an experimental entry-counting wrapper for the stock
`0x00074419` hook. For `.0065` it appends:

- wrapper at `0x000d0f90`, compact Thumb pointer `0x000d0f91`;
- telemetry at `0x000d0fac`: 32-bit count followed by captured `r0` through `r3`;
- tail branch back to the stock hook at `0x000d060c`.

Generated firmware is `work/sdk0065_hook_counter.bin`; the Pico probe is
`work/hardware-hook/picow_hook_counter.elf`. The embedded firmware array is 7028
bytes, matching the generated container rather than the 6970-byte stock image.
On successful HCI initialization, the probe requests connectable advertising as
`ISO Hook Probe` and reports counter changes through USB serial. It now exits
on HCI startup failure and suppresses telemetry until `HCI_STATE_WORKING`.

The image was flashed and failed controller startup with error `180`. An
append-only control using the original hook table reached HCI working state in
the earlier run, while a one-byte config-name change also failed. Configuration
signature rejection is now established by ROM analysis, a fresh name-only
control, and offline verification; see `analysis/config-validation.md`.
The wrapper's runtime behavior and free-space placement remain unvalidated;
instrumentation also adds execution time even if it preserves register values.

## Runtime command-lookup instrumentation

`picow_runtime_probe` proves execution of a controller-side Thumb-2 wrapper
after authenticated configuration startup. It boots the stock `.0065` records
with an explicitly zero-filled append region, checks stock HCI version, the
callback pointer and its original instructions, then verifies every backplane
write by reading it back. The SDK write API alone is insufficient: it returns
zero even after a bus write error.

Clean ROM `0x0006cf60` decodes HCI OGF/OCF. At `0x0006cfbc..0x0006cfca`, it
loads callback `word(0x00203a34)` and calls it with command object in `r0`,
descriptor output in `r1`, OCF in `r2`, and OGF in `r3`. Stock installs Thumb
pointer `0x000d035d`. The probe temporarily replaces that aligned pointer with
`0x000d0f91`; the wrapper records entry registers and tail-calls unchanged
stock code at `0x000d035c`. Registers and APSR are preserved.

Retained live result `work/runtime-hook-probe/runtime-hci-pass.txt`:

```text
baseline complete=1 stock-version=1
scratch-write-restore=pass
callback-write=pass
instrumented complete=1 stock-version=1
count=1 r0=0020a1e4 r1=00217780 ocf=00000001 ogf=00000004
callback-restored=1
restored complete=1 stock-version=1
scratch-restored=1
execution=pass
```

The command is HCI Read Local Version (`0x1001`); the count and decoded fields
prove wrapper execution, while its unchanged valid response checks the tail
call. Initial stock-only probes stopped before writes because the proposed
scratch bytes were nonzero. Explicitly appending zero bytes made the strict
zero/stability guard pass; the probe does not assume stock initializes spare
Patchram. This establishes only the tested temporary use of this region.

`hardware/runtime_hook.S` is controller code, not RP2040 code.
`python3 analysis/check-runtime-probe.py` assembles it and compares exact bytes
with the embedded wrapper, checks dispatch ABI bytes against the clean ROM,
and verifies the original stock callback prefix. `--prepare` also creates
`work/runtime-hook-probe/zero-append.bin` and its custom firmware header,
preserving every original downloaded byte and the signed configuration.

Stock HCI firmware was restored through SWD with `Verified OK` and two fresh
HCI capability passes after the experiment; retained logs are
`work/runtime-hook-probe/restore-after-hci.log` and
`work/runtime-hook-probe/stock-after-hci.txt`. This result establishes code
execution, not scheduler lifecycle behavior or ISO support.

## Live state-7 experiment

`STATE7_PROBE=ON` extends the command probe with a temporary inline branch at
`0x000d060c`. It first verifies the original `push.w {r4-r8,lr}` bytes, installs
a 56-byte wrapper in the zero-filled append region, then replaces that four-byte
instruction with an aligned `b.w`. The wrapper preserves registers and flags,
records entry registers, replays the displaced push, and resumes at
`0x000d0610`. No signed boot configuration changes.

The control (`STATE7_CONTROL=ON`) performs identical setup and traffic while
writing back the original prologue instead of the branch. The host discovers
and connects only to the public Pico address printed by the probe. Each run
observes 35 seconds, stops advertising/disconnects, restores the prologue,
checks HCI, and restores scratch. SWD then restores the stock HCI probe.

| Observation | Control | Instrumented |
| --- | --- | --- |
| State-7 counter | 0 | 1 |
| First counter sample | 3096 ms | 3056 ms |
| Successful connection | 4569 ms, handle `0x0040` | 4533 ms, handle `0x0040` |
| Successful disconnection | 8747 ms, reason `0x13` | 8814 ms, reason `0x13` |
| Prologue/scratch restored | yes | yes |
| Post-restore HCI version check | pass | pass |
| Final result | `control-pass` | `pass` |

Times are RP2040 milliseconds since boot within each run. Counter 1 is observed
after the advertising parameter/data/enable sequence and before connection;
these captures do not isolate which individual setup command triggers entry.
Entry values are `r0=r1=0x00207588`, `r2=0`, `r3=0x0004ce03`.
Thus this hook is not connection-only. Neither connection nor disconnection
adds another entry during this window.

The post-entry sample at object `+0x154` (`0x002076dc`) starts with bytes
`01 02`, matching the dispatcher state write and hook flag. Derived halfwords
`+0x170/+0x174=27`, `+0x172/+0x176/+0x178/+0x17c=328`, and `+0x17e=2704`
match the recovered min/max equations; `+0x18c=744`. This is a subsequent
backplane snapshot, not an atomic before/after trace or proof of clock units.
Entry-register snapshots also lack synchronization with controller writes.

Advertising disable returns `0x0c` after connection automatically stops legacy
advertising. Cleanup accepts only that status after a validated peripheral
connection, its matching successful disconnection, no outstanding/uncertain
connection, and a fresh successful HCI check. Other failures retain executable
bytes. The initial instrumented capture had disconnect reason `0x08` and
stopped conservatively at this cleanup case; it is retained as
`work/runtime-hook-probe/state7-initial.txt`. The final control and instrumented
runs both have reason `0x13`; this small sample does not establish radio timing
or reliability equivalence.

Runnable checks:

```sh
python3 analysis/check-runtime-probe.py --capture work/runtime-hook-probe/state7-control.txt
python3 analysis/check-runtime-probe.py --capture work/runtime-hook-probe/state7.txt
```

| Retained artifact | SHA-256 |
| --- | --- |
| `work/runtime-hook-probe/runtime-hci-pass.txt` | `a71f4e2f365024fc7576321d54644524b420ea20cd73094d7cd336189b445251` |
| `work/runtime-hook-probe/state7-control.txt` | `4b0934d9907cadafc2be71391192479202cd5fe96572525a6185c65f778132dc` |
| `work/runtime-hook-probe/state7.txt` | `dfb3518918d4d4d8f997c645483ada6e26c2cb2a1a17e3416b82439a767d9129` |
| `work/runtime-hook-probe/stock-restored.txt` | `4eab5268f1f73c24986328ba61a2d091bf308a0ce7cf755a1969d98dd8b1e78d` |
| `work/runtime-hook-probe/zero-append.bin` (56-byte reservation) | `4b6642cae86ff31588d9373ed40cec5990867d3240ad28fc04f8a8cb7d3aedb6` |

Final restoration log `work/runtime-hook-probe/restore-stock.log` reports
`Verified OK`; two live stock checks still report ISO support absent. Host
discovery was stopped. Scheduler registration and payload programming are
mapped in `analysis/iso-event-path.md`; safe allocation, event ownership and
timing remain unresolved before inserting an inert event or building CIS/BIS.

## Live phase callback follow-up

`EVENT_PHASE_PROBE` now clones the stock event callback table and instruments
only its `+0x10` phase callback. Live control stays at zero; phases 0/1/2 each
reach 93 in the instrumented connection. Both runs disconnect with reason
`0x13`, restore the global table pointer, and pass HCI. Published storage stays
resident until controller reset because event objects retain table pointers.
See `analysis/event-phase-probe.md` for exact evidence and limitations,
`analysis/event-ownership.md` for the ownership/registration contracts, and
`analysis/event-timing.md` for timing/accessor findings. Independent radio-event
construction and completion still gate ISO implementation.
