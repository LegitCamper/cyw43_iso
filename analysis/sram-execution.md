# Resident SRAM allocation and execution probe

`SRAM_EXEC_PROBE=ON` tests ownership and execution of a 512-byte resident SRAM
allocation through the already validated HCI command-lookup callback. It adds
no radio event or connection. Build, assembler checks, independent ABI review,
and the live allocation/execution test pass. Allocated SRAM now provides a
validated home for this small controller callback, beyond the patch append.

## Allocation guard and lifetime

The ordinary HCI wrapper experiment runs and restores stock first. Host then
reads allocator globals at `0x00200560`, requiring initialized byte 1, aligned
cursor/end, `0x00209bac <= cursor <= 0x0021de00`, and
`cursor + 512 < end <= 0x00220000`. The upper cursor bound keeps allocation
below retained configuration starting at `0x0021e000`. Addition cannot wrap
after the cursor bound check. Size is fixed and already four-byte aligned.

Host checks the 112-byte patch reservation is zero and stable, writes the
allocation wrapper plus expected cursor/end with readback, then publishes it
at HCI callback pointer `0x00203a34`. Inside the controller, the wrapper saves
PRIMASK and masks interrupts before checking its one-shot latch, `IPSR == 0`,
initialized byte 1, and exact equality with the host-validated cursor/end.
Changed bounds reject allocation. This closes the maskable-interrupt gap
between host validation and allocation; NMI/HardFault are outside that claim.

Only a passing guard calls `0x00017856(512)`. Its nested interrupt save/restore
keeps the outer mask intact. The wrapper restores original PRIMASK, input
registers, and APSR, then tail-calls stock lookup `0x000d035c`. It saves
`r0-r9,r12,lr` in an aligned 48-byte frame. The nonzero latch is set before
allocation; rejected or null-return attempts leave status 1. Successful calls
replace status with the returned pointer. A second HCI query checks one-shot
behavior: the sampled arena cursor must have advanced exactly 512 bytes.

| Controller address | Contents |
| --- | --- |
| `0x000d0f90` | 96-byte allocator wrapper, literals, padding |
| `0x000d0ff0` | One-shot status/result |
| `0x000d0ff4` | Host-validated expected cursor |
| `0x000d0ff8` | Host-validated expected end |
| `0x000d0ffc` | Reserved zero word |

After allocation, host restores stock lookup and verifies a fresh HCI response.
It requires the returned pointer to match the expected cursor, exact cursor
advance, unchanged end, and all 512 reserved bytes zero before writing code.
No fixed-pool release or cursor rewind is used. Scratch and arena storage
remain resident until controller reset on every path after scratch writes.

## Executable payload and evidence

The 52-byte SRAM payload uses `ADR` for its telemetry at allocation `+32` and
a literal `LDR pc` to stock Thumb target `0x000d035d`; no dynamic relocation
is needed. It increments a counter, records original `r0-r3`, and preserves
input registers/APSR. Host writes/readbacks the payload, publishes its Thumb
pointer, requests local version, then restores stock and verifies HCI again.
One count with OCF 1 and OGF 4 plus a valid stock response is the success test.

Execution permissions and cache/memory behavior are not established by static
allocation analysis. Failed SRAM execution may halt the controller; the
hardware runner must restore stock by SWD in `finally`. Initialization, HCI,
or restoration failure is not success. IRQ-mask latency is not measured.
This bounded test cannot establish general allocator safety, new event
insertion, radio scheduling, ISO transport, or LE Audio support.

## Reproduction

```sh
python3 analysis/check-runtime-probe.py --prepare --sram
PICO_SDK_PATH=/var/home/sawyer/projects/pico-sdk cmake -S hardware -B work/hardware-sram-exec \
  -DCYW43_CUSTOM_BTFW_DIR="$PWD/work/sram-exec-probe/custom-fw" -DSRAM_EXEC_PROBE=ON
PICO_SDK_PATH=/var/home/sawyer/projects/pico-sdk cmake --build work/hardware-sram-exec --target picow_runtime_probe -j4
python3 analysis/check-runtime-probe.py --sram --capture work/sram-exec-probe/sram-instrumented.txt
```

SRAM and lifecycle modes require separate builds. The prepared image uses the
same validated 112-byte zero append as the phase probes. The checker matches
embedded assembly, branch targets, metadata offsets, and identical payload
bytes linked at two SRAM bases. Capture checks require both allocation calls,
validated bounds/cursor advance, execution telemetry, and stock restoration.
Allocator ROM contracts and corrected startup captures are in
`radio-event-contract.md`; no old oversized RAM dump supplies bounds here.

## Live result

Capture `work/sram-exec-probe/sram-instrumented.txt` records:

- Initial cursor `0x00217e78`, end `0x00220000`, initialized byte 1.
- Allocation `0x00217e78`; cursor `0x00218078` after two HCI queries, end unchanged.
- SRAM callback count 1, OCF `0x00000001`, OGF `0x00000004`, valid stock version.
- Callback restored, fresh stock version valid, result `pass`.

Successful allocation also establishes that its in-controller `IPSR == 0` guard
passed for this HCI lookup. It does not prove that a blocking scheduler API may
be called safely from that context. The 512-byte allocation and 52-byte callback
do not establish execution permissions for every SRAM region or payload size.

Runner `work/sram-exec-probe/run.py` then restored stock through the selected
SWD probe. Its restoration log contains `Verified OK`; the restoration capture
contains two HCI capability passes, subversion `0x2310`, ISO absent. Host
discovery remains false. Stock reset is the reclamation boundary for this run.

| Artifact under `work/sram-exec-probe/` | SHA-256 |
| --- | --- |
| `zero-append.bin` | `b3c0c99774135c32beea47af847838077a1f0b7a6be661a68ddcd3641362ad7d` |
| `sram-instrumented.txt` | `20967dce83100d6f5b6feb3f94e9b9e4627a67dbf635a7eafe9fb7ef7728d0db` |
| `sram-instrumented-stock-restored.txt` | `4eab5268f1f73c24986328ba61a2d091bf308a0ce7cf755a1969d98dd8b1e78d` |
