# Live scheduler phase callback

Control and instrumented runs on CYW43439 `.0065` both pass. The instrumented
run observes phases 0, 1, and 2 during a successful BLE connection. This proves
the stock scheduler calls appended controller code through a cloned callback
table. It does not implement ISO or validate a new event's radio timing.

## Implementation and lifetime

`EVENT_PHASE_PROBE=ON` in `hardware/picow_runtime_probe.c` verifies the stock
global `0x00202fb8 == 0x0008af40` and all eleven ROM table words. It checks the
112-byte reservation is zero and stable, writes an assembled Thumb wrapper and
a table clone, verifies every write by readback, then publishes the clone.
Only table offset `+0x10` changes, from `0x0004917f` to `0x000d0f91`.
`STATE7_CONTROL=ON` publishes the identical clone with the stock callback.

| Address | Size | Contents |
| --- | --- | --- |
| `0x000d0f90` | 48 bytes | Wrapper, literal and padding |
| `0x000d0fc0` | 20 bytes | Phase 0/1/2/other counters, last event pointer |
| `0x000d0fd4` | 44 bytes | Cloned callback table |

The region ends at `0x000d1000`. The wrapper preserves registers and APSR,
increments one bounded bucket, records the event pointer, and tail-branches
to stock `0x0004917e`. It makes no additional controller clock/MMIO calls.
The common HCI callback experiment runs and restores its changes first.

After a validated connection/disconnection and fresh HCI barrier, the probe
restores the global table pointer and checks HCI again. Once publication is
attempted, wrapper and clone remain resident on every cleanup path. Embedded
events retain table pointers even after disconnect; resetting the controller
is the reclamation boundary. See `event-ownership.md`.

## Observed results

| Observation | Control | Instrumented |
| --- | --- | --- |
| First sample | All zero at 3111 ms | All zero at 3109 ms |
| Successful connection | 5918 ms, handle `0x0040` | 9853 ms, handle `0x0040` |
| First nonzero sample | None | 9898 ms, counts `1/1/1/3` |
| Last counts, phases 0/1/2/other | `0/0/0/0` | `93/93/93/279` |
| Successful disconnection | 10872 ms, reason `0x13` | 14383 ms, reason `0x13` |
| Last changed sample | Initial zero sample | 14386 ms |
| Global restored / HCI check | Pass | Pass |
| Final result | `control-pass` | `pass` |

Every nonzero event pointer is `0x00207730`, matching the known static
connection owner `0x00207588 + 0x1a8`. Host discovery stops after each peer
run. Advertising disable returns `0x0c` after the connection has automatically
stopped advertising; the existing validated disconnect/barrier policy applies.

Times are RP2040 observation milliseconds. Counters and last-event storage are
asynchronous and lack snapshot coherence. Nested calls could lose increments;
exact callback rates and cross-field equality are not guaranteed. The `other`
bucket does not identify individual phase values. The final increment sampled
after HCI disconnection does not prove callback execution occurred after that
event. Two runs establish neither timing bounds nor reliability equivalence.
Controller clock and accessor contracts are in `event-timing.md`.

## Reproduction and evidence

Build commands are in the README. Control and instrumented builds succeed;
the checker verifies assembled bytes, clean-ROM ABI/table, and both captures:

```sh
python3 analysis/check-runtime-probe.py --phase --capture work/event-phase-probe/phase-control.txt
python3 analysis/check-runtime-probe.py --phase --capture work/event-phase-probe/phase-instrumented.txt
```

Hardware runner `work/event-phase-probe/run.py` uses the selected debug probe
serial `E6647C74039D512C` and always restores stock in `finally`. Both restoration
logs report `Verified OK`; both live captures contain two stock HCI capability
passes with subversion `0x2310` and ISO absent. Final host `Discovering` is false.

| Artifact under `work/event-phase-probe/` | SHA-256 |
| --- | --- |
| `zero-append.bin` | `b3c0c99774135c32beea47af847838077a1f0b7a6be661a68ddcd3641362ad7d` |
| `phase-control.txt` | `d4f47857ecc87c4ef312e4fc5be97f3940f68c52b53a4f58ff8c6e960ca6005d` |
| `phase-instrumented.txt` | `49819aa854b8f3a8c536fac157513a37bd4a15a2a5d407a3f9618f88a4be03eb` |
| `phase-control-stock-restored.txt` | `89527d5cae2b377f1746cf71b05b216a2014c281995498d54756193228d52e21` |
| `phase-instrumented-stock-restored.txt` | `4eab5268f1f73c24986328ba61a2d091bf308a0ce7cf755a1969d98dd8b1e78d` |

The earlier 56-byte runtime image and its captures remain unchanged. Remaining
ISO work requires safe independent radio-event construction, scheduling and
completion, followed by CIS/BIS link-layer behavior, ISOAL, and HCI ISO transport.
The separate software timer found in ROM provides a smaller periodic-execution
experiment, but does not supply radio-event ownership or timing.
