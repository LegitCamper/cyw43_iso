# Independent controller timer execution

`CONTROLLER_TIMER_PROBE=ON` builds a bounded periodic software-timer experiment
on top of the guarded resident 512-byte SRAM allocation. Live execution passes.
This timer does not schedule radio events or implement CIS, BIS, ISOAL, or audio.

`hardware/controller_timer.S` preserves the HCI callback's registers and APSR,
saves PRIMASK, masks interrupts, and rejects nonzero IPSR before timer control.
It initializes fresh resident timer storage once with ROM `0x742c`, callback,
argument and period 125000 timer ticks; arms through `0x73d4`; and cancels through
`0x7408`. The stock lookup target remains `0xd035d`. Periodic dispatch requeues
before calling the callback, per `event-ownership.md`; cancellation therefore
must unlink even after earlier callbacks have completed. Timer ticks are not
claimed calibrated microseconds.

The 256-byte position-independent payload starts at allocation +64. Its trace
is payload +192 and its 32-byte timer is payload +224. Trace contains command,
status, callback count, callback IPSR, lateness, callback argument, cancellation
result and a reserved word. Status 0 permits initialization, 2 permits
cancellation, and 3 prevents further control. Repeated HCI arm queries do not
reinitialize linked storage. The callback only updates telemetry.

Host observes two 500ms windows without HCI polling, cancels via a fresh HCI
query, then checks a third 500ms window. Passing requires increasing counts,
nonzero callback IPSR, exact argument, cancellation return zero, status 3,
inactive timer flags, unchanged final count and restored stock HCI. Storage
remains resident until reset, including on failure. External runner restores
stock firmware through SWD in `finally` and verifies two capability queries.

## Live evidence

Capture `work/controller-timer-probe/timer-instrumented.txt`:

- Allocation `0x217e78`; payload `0x217eb8`; trace `0x217f78`.
- Counts 4 then 8 during the two independent observation windows.
- Callback IPSR 34, matching external IRQ 18 from the ROM timer vector.
- Argument `0x217f78`; latest lateness 5 timer ticks.
- Cancellation return 0, flags `0x3`, count 8 then 8 after cancellation.
- All arm, repeated arm, cancel, and restored HCI queries passed.
- Stock reset, SWD `Verified OK`, two stock HCI capability passes completed.

Capture SHA-256:
`a9fb57ecce0deaccaf3f8195c503f58099825b8d485257a4bec2ca0384e7583b`.
Stock-restored capture SHA-256:
`4eab5268f1f73c24986328ba61a2d091bf308a0ce7cf755a1969d98dd8b1e78d`.

REA's configured Ghidra provider remains unavailable (`GHIDRA_INSTALL_DIR`
unset). This pass reuses the existing local Ghidra/objdump timer evidence and
checks the exact clean-ROM SHA-256. These captures are local runtime evidence,
not REA Evidence IDs. No new REA native session was opened.

## Reproduction

```sh
python3 analysis/check-runtime-probe.py --prepare --sram
PICO_SDK_PATH=/var/home/sawyer/projects/pico-sdk cmake -S hardware -B work/hardware-controller-timer \
  -DCYW43_CUSTOM_BTFW_DIR="$PWD/work/sram-exec-probe/custom-fw" -DCONTROLLER_TIMER_PROBE=ON
PICO_SDK_PATH=/var/home/sawyer/projects/pico-sdk cmake --build work/hardware-controller-timer --target picow_runtime_probe -j4
python3 analysis/check-controller-timer.py --capture work/controller-timer-probe/timer-instrumented.txt
```

`work/controller-timer-probe/run.py` performs flashing, capture and stock
restoration; retained capture paths must be preserved before another run.
The checker compares embedded bytes with assembly linked at two SRAM bases,
verifies layout and ROM identity, and checks requested live checkpoints.

The measured IRQ context cannot call the blocking scheduler registration API.
Timer cancellation does not establish general scheduler event reclamation;
resident storage and controller reset remain the ownership boundary.
