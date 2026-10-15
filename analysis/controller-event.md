# Resident independent scheduler event

`CONTROLLER_EVENT_PROBE=ON` validates one independent compact scheduler event
on the attached CYW43439. Registration returns through the HCI caller;
completion runs once, asynchronous removal reaches cleanup once, the object
is zeroed, checked scheduler references are clear, and stock HCI remains healthy.
This establishes a scheduler lifecycle, not transmission of any radio packet.

## Implementation and ownership

The probe reuses the entire guarded 512-byte resident allocation only after
the SRAM execution callback has returned and its stock callback pointer has
been restored. Event and timer probes require separate builds. No code is
placed at allocation +64 for this mode: that would exceed the reservation.

| Allocation offset | Contents |
| --- | --- |
| `+0` | Position-independent HCI entry and completion/cleanup callbacks |
| `+352` | Private 44-byte callback table |
| `+400` | Eight-word telemetry |
| `+432` | Private `0x28`-byte event |

Controller entry requires an unused latch, IPSR 0, PRIMASK 0, nonnull current
thread pointer from `0x201fe4`, null registration/removal override pointers
`0x208e28` and `0x208e38`, and an empty pending slot `0x2042cc`. The normal
registration API also checks the pending slot under its own lock. These
preconditions cover this experiment; they are not a general concurrent
registration interface or a guarantee against firmware changing hooks later.

The event uses selector 0, type `0x30`, priority `0x71`, classification 2,
and deadline `((clock(0) & ~1) + 32) & 0x07ffffff`. The clock read preserves
the constructor's outer lock `0xb4ae` / `0xb1de` and interrupt masking.
Registration `0x1c61e(event, table, 3)` executes with interrupts enabled.
No stock clock-adjustment constructor or completion callback is called.

Table phase callback `0x3113f` and null accessor `0x31207` retain the compact
stock event contract. Custom completion first checks current-event identity
through `0x1cf4e` and ready state through `0x1cb1c`, records telemetry, then
acknowledges via `0x1d5b8` and requests asynchronous removal through `0x1c97e`.
Cleanup increments an external count before tail-calling stock `0x31138`,
which clears exactly the 40-byte event. An additional HCI query checks the
one-shot latch without rearming. Code, table, telemetry and event remain
resident until controller reset, even after observed cleanup.

## Blocking caller evidence

Existing ROM evidence shows registration ending in `0xc37c`, which arms a
timeout timer and calls `0x173f4(..., 3, 1, 0xffffffff)`. The latter calls
`0x1a540`: its wait path loads the current-thread pointer from `0x201fe4`,
updates thread wait state, and may suspend through `0x603c0`. Thus IPSR 0
alone did not previously prove the injected HCI context compatible with
this blocking operation. The live result below supplies that bounded proof.

The stock lock at `0xb4ae` is patch-redirected through `0xd02a4` to `0xd0ef0`;
the probe calls its public entry and preserves this patch path. Local objdump
inspection covered these exact ROM/patch regions. REA's Ghidra provider was
unavailable because `GHIDRA_INSTALL_DIR` is unset; no REA Evidence IDs or new
native session are claimed. Source contracts and exact clean-ROM identity
remain those in `radio-event-contract.md` and `event-ownership.md`.

## Live result

Final capture: `work/controller-event-final-probe/event-instrumented.txt`.

- Allocation `0x217e78`, event `0x218028`, deadline `0x284`.
- Entry IPSR 0, PRIMASK 0, current-thread pointer `0x206dc4`.
- Registration-return status 2; completion count 1; cleanup count 1.
- Completion IPSR 27, corresponding to external IRQ 11.
- Entire 40-byte event zero after cleanup.
- Current, candidate, incoming, pending and iteration-cursor pointers zero.
- Extra references `0x202bf8` / `0x202bfc` and phase owner `0x204cc8` zero.
- Repeated lookup preserves telemetry; stock HCI restoration succeeds.
- Final SWD stock restore reports `Verified OK`; two HCI capability passes
  report stock subversion `0x2310` and absent controller ISO support.

Capture SHA-256:
`2075141daaa116ca37aaeee0048537de5e5cdd87bad8e3470182b31571d84eae`.
Stock-restored capture SHA-256:
`4eab5268f1f73c24986328ba61a2d091bf308a0ce7cf755a1969d98dd8b1e78d`.

The earlier capture under `work/controller-event-probe/` has the same observed
lifecycle without the expanded reference log. The final run uses final rebuilt
source, including the phase-owner acceptance check. Source and ELF digests
are retained in `work/controller-event-final-probe/source-and-elf.sha256`.

## Reproduction and limits

```sh
python3 analysis/check-runtime-probe.py --prepare --sram
PICO_SDK_PATH=/var/home/sawyer/projects/pico-sdk cmake -S hardware -B work/hardware-controller-event \
  -DCYW43_CUSTOM_BTFW_DIR="$PWD/work/sram-exec-probe/custom-fw" -DCONTROLLER_EVENT_PROBE=ON
PICO_SDK_PATH=/var/home/sawyer/projects/pico-sdk cmake --build work/hardware-controller-event --target picow_runtime_probe -j4
python3 analysis/check-controller-event.py --capture work/controller-event-final-probe/event-instrumented.txt
```

The retained `work/controller-event-final-probe/run.py` flashes the experiment,
captures bounded output and restores stock in `finally`; preserve existing
captures before rerunning. Checker verifies assembly at two SRAM bases,
embedded bytes, layout, selected ROM anchors and all reported checkpoints.
Current cleanup attempts a bounded stock-version query even if callback
restoration readback fails; success still requires verified restoration.
External SWD restoration remains mandatory on every outcome.

Still unresolved: radio access address, channel/PHY setup, packet header,
payload/DMA ownership, CRC, TX/RX completion, subevent spacing, recurring event
rearming, collision behavior with active connections, and scheduler jitter.
No over-air packet, CIS/BIS establishment or interoperable LE Audio was tested.
Cleared observed references do not prove every scheduler reference drained;
storage remains allocated until reset rather than being freed or reused.
