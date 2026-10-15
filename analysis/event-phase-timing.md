# Phase-2 stored-clock delta probe

`EVENT_PHASE_TIMING=ON` calls stock phase callback `0x0004917e`, then captures
one modular difference after phase 2 returns:

```text
delta27 = (word(event + 0x1c) - word(event + 0xa8)) & 0x07ffffff
```

This is deliberately scoped telemetry, not a ring or a complete event snapshot.
It adds no clock-getter, MMIO, radio, or scheduler calls. Build, static checks,
and live control/instrumented runs pass. The instrumented run observes delta 1;
this does not establish independent event timing or ISO support.

## Reservation and lifetime

| Address | Bytes | Contents |
| --- | --- | --- |
| `0x000d0f90` | 64 | Thumb wrapper, literal, padding |
| `0x000d0fd0` | 4 | Latest delta; `0xffffffff` means no observation |
| `0x000d0fd4` | 44 | Callback table clone |

Reservation ends at `0x000d1000`, matching the validated phase counter probe.
The prepared zero-append image is byte-identical to that probe's image; only
post-boot writes differ. Installation checks stable zero RAM, the global stock
pointer, all eleven stock table entries, and every write by readback. The
sentinel lies outside the 27-bit result domain, so even delta zero establishes
execution. Only callback table `+0x10` changes. Control mode publishes the same
clone with the stock callback and requires the sentinel to remain untouched.

Once publication is attempted, scratch remains resident until controller reset,
because event objects retain callback pointers. The existing bounded connection,
disconnect, HCI barrier, global restoration, and stock recovery policy applies.

An earlier 504-byte reservation extending through `0x000d1188` failed controller
initialization with error 179 (`CYBT_ERR_TIMEOUT` in the SDK shared-bus driver)
in the stock-callback control. No timing wrapper executed. This suggests a
loading or memory-boundary constraint; it does not
establish the cause. Failed image, header, control ELF, and captures remain under
`work/event-phase-timing/`. Compact artifacts use separate directories.

## ABI and evidence limits

The wrapper saves `r4-r8,lr` in a 24-byte frame, preserving 8-byte stack alignment.
Non-flag-setting moves retain event and phase before calling stock. Input
`r0-r3`, `r12`, and APSR reach stock unchanged. After return, `r8` saves APSR
before the phase check. Telemetry leaves stock return `r0-r3` and `r12` untouched;
APSR, callee-saved registers, and caller return address are restored. This is
specific to the established two-argument callback ABI, not a generic trampoline
for stack arguments. Independent assembly review found no ABI defect.

The telemetry store and host sample are single aligned words. Each sampled delta
is coherent, but the two event-field loads are not an atomic snapshot: an
interrupt can change fields between loads. Other event/selector identity is not
captured. Phase 2 normally stores its selected clock at `+0xa8`, but conditional
paths can leave that field unchanged. Consequently a delta does not prove a
fresh clock sample, lateness, callback duration, or deadline feasibility.

Host prints only changed values, polls every 10 ms, and can miss intermediate
updates. No exact phase rates or timing bounds follow. Instrumentation adds
latency and 24 bytes of stack use on every callback phase. Native and selected
clocks use different domains; see `event-timing.md` for static contracts.

## Build and check

```sh
python3 analysis/check-runtime-probe.py --prepare --timing
PICO_SDK_PATH=/var/home/sawyer/projects/pico-sdk cmake -S hardware -B work/hardware-phase-timing-compact \
  -DCYW43_CUSTOM_BTFW_DIR="$PWD/work/event-phase-timing-compact/custom-fw" \
  -DEVENT_PHASE_TIMING=ON -DSTATE7_CONTROL=ON
PICO_SDK_PATH=/var/home/sawyer/projects/pico-sdk cmake --build work/hardware-phase-timing-compact --target picow_runtime_probe -j4
```

Run the control first. Change `STATE7_CONTROL=OFF` and rebuild for instrumented
execution. Capture tags are `PHASE_TIMING`; advertising name remains
`ISO State7 Probe` for the existing peer runner. Success requires at least one
non-sentinel delta and a validated connection/disconnection. Control success
requires no delta updates and a validated connection/disconnection.

```sh
python3 analysis/check-runtime-probe.py --timing --capture work/event-phase-timing-live/timing-control.txt
python3 analysis/check-runtime-probe.py --timing --capture work/event-phase-timing-live/timing-instrumented.txt
```

The checker compares embedded bytes with assembled source, validates the stock
callback ABI/table, stock branch destination, delta mask, store instruction,
reservation bounds, and capture checkpoints. Existing `--phase` and state-7
modes remain compatible. This probe does not construct radio events or implement
ISO/LE Audio.

## Live results

| Observation | Control | Instrumented |
| --- | --- | --- |
| Successful connection, RP2040 ms | 4877 | 4040 |
| Handle | `0x0040` | `0x0040` |
| Changed telemetry samples | None; sentinel retained | Delta `1` at 4052 ms |
| Successful disconnect, RP2040 ms | 9419 | 8774 |
| Global pointer restored and fresh HCI query | Pass | Pass |
| Result | `control-pass` | `pass` |

The host printed only one changed value in the instrumented run. Repeated stores
of the same value do not produce more lines; this is not a count of callbacks.
Delta 1 is a difference in the selected-clock domain, not a measured one-slot
execution time or a safe scheduling budget.

The first compact control booted and restored cleanly but produced `unproven`
because the host peer connection failed with `le-connection-abort-by-local`.
Its capture and a subsequent unsuccessful peer retry remain under
`work/event-phase-timing-compact/`. The successful fresh control and instrumented
captures are under `work/event-phase-timing-live/`; their runner uses the retained
control/instrumented ELFs from the compact directory. No failed evidence was
overwritten.

Each run's runner restored stock in `finally`, verified it through SWD, and
captured two stock HCI capability passes with subversion `0x2310` and ISO absent.
Host discovery was false after both successful runs. The failed expanded control
also has verified stock restoration and two HCI passes under its own directory.

| Artifact under `work/` | SHA-256 |
| --- | --- |
| `event-phase-timing-compact/zero-append.bin` | `b3c0c99774135c32beea47af847838077a1f0b7a6be661a68ddcd3641362ad7d` |
| `event-phase-timing-live/timing-control.txt` | `4d4aa159ca43e55a50230e6fe4808138adbfe454717b64ea13e5ef413617f932` |
| `event-phase-timing-live/timing-instrumented.txt` | `d5312195517848cb0e605c1380dcecd8c6b7e5d401e452b3092226c82afd6a40` |
| `event-phase-timing/zero-append.bin` | `52e27063fcdc940350e1a780532685803aa83b08978b61526ce272edfff8d218` |
| `event-phase-timing/timing-control.txt` | `3cc61bef758f8c1954c7bb52a505bc291312c9b312eed536b101831ba859b50f` |
