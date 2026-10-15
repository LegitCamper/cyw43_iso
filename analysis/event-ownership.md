# Event ownership and registration

Static evidence only; no custom event insertion or ISO execution established.
Subsequent live [timer](controller-timer.md) and [compact event](controller-event.md)
probes validate the bounded cases described there; ISO execution remains absent.
Input: `work/cyw43439-btrom-clean.bin`, SHA-256
`d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038`.
Evidence: `work/ghidra/inspect_event_ownership.txt`,
`work/ghidra/inspect_clean_event_timing.txt`,
`work/ghidra/inspect_clean_event_timing2.txt`, and
`work/ghidra/clean_event_ownership_disassembly.txt`. These are local
Ghidra/objdump artifacts, not REA Evidence IDs or runtime observations.

## LE events belong to connection pool objects

`0x00046ede(index)` is a tail call to `0x0004da46`, not an allocator.
The latter requires a nonnull table at `word(0x002030c0)` and an index below
`byte(0x002030ad)`. It returns null for a null slot, otherwise
`table[index] + 0x1a8`. `0x0004d9d6(index)` returns the same slot without
the offset. `0x0004da24(index)` returns the slot plus `0x348`.

The LE initializer `0x00047286` clears `0x1a0` bytes at the event address.
Thus this particular event occupies owner offsets `[0x1a8, 0x348)`.
Its `+0x50` points back to its owner; payload queues are embedded at event
`+0xe4` and `+0xfc`, with pointers at `+0xdc` and `+0xe0`.
This is not a generic scheduler-object size guarantee.

Pool installation at `0x0004d744(pool)` stores its argument at
`0x002030c0`, clears the static owner at `0x00207588` for `0x370` bytes,
and sets the additional-owner pool pointer at `0x002030bc` to
`pool + 4 * byte(0x002030ae)`. `0x0004db1c(index)` checks slot bounds,
prefers the static owner when owner byte `+0x3c` bit 0 is clear, otherwise
searches additional owners with stride `0x370` for that same clear bit.
It assigns the selected pointer to the slot and clears only the first
`0x1a4` bytes. No per-event heap allocation occurs in these functions.
The source/lifetime of the externally installed pool is not resolved here.

Owner release `0x0004dd0a(index)` requires an existing owner with byte
`+0x3c` bit 0 set. Its normal path releases related resources, clears the
first `0x1a4` owner bytes at `0x0004ddb0`, then nulls its table slot at
`0x0004ddc4`. It does not clear the event at `owner + 0x1a8`.
Consequently a null connection slot or cleared owner flag does not prove
that an event's old callback-table pointer disappeared or that scheduler
references were drained. Individual pool storage is reused, not freed here.

## Registration checks

`0x0001c61e(event, callbacks, mode)` first permits the hook at
`word(0x00208e28)` to intercept the operation by returning nonzero.
The ordinary path rejects any of:

- Null event, null table, or null callback at table `+0x28`.
- Event state byte `+0x14 == 14`; helper `0x0001bfdc` tests exactly this.
- Nonzero IPSR; `0x00085f58` is `mrs r0, ipsr; bx lr`.
- Occupied pending registration slot `word(0x002042cc)`.
- Event classification byte `+0x13` equal to 0 or 4.

On acceptance it stores the table pointer directly at event `+0x00`, sets
state byte `+0x14 = 10`, sets mode bits `+0x16[1:0]`, queues the object,
publishes it in `0x002042cc`, restores interrupt masking and releases its
outer lock, then calls `0x0000c37c`. There is no explicit boolean success
return contract in this implementation. Modes 1 through 3 survive;
`0x0001c330` maps other inputs to mode 1.

A zero-filled object fails the `+0x13` check. The normal LE initializer
calls `0x00046e4a`, which sets `+0x13 = 2`, clears `+0x81`, and stores
its second argument at `+0x1c`. These assignments do not establish that
arbitrarily setting those fields produces a safe standalone event.

## Intrusive queue ownership

`0x0000c5de(event)` uses the doubly linked node at event `+0x04` and
priority byte `+0x12`. Its list sentinel is `0x00202320`.
It returns immediately when queue-state byte `+0x15 == 3`; for other
nonzero queue states it first calls `0x0000c57e` to unlink the node.
It then sets `+0x15 = 4` and inserts by descending priority, after
existing equal-priority objects. Insertion helpers `0x0005ede2` and
`0x0005ede6` write both node links and neighboring links.

`0x0000c57e` updates scheduler cursor/current-candidate references,
performs an additional operation for queue state 1 when event state is
not 14, unlinks the node, and clears `+0x15`. Its unlink helper
`0x0005edf4` dereferences both links without validating them and does
not clear the removed node's links. Calling it on an unqueued,
zero-filled object is unsafe even though a diagnostic can be emitted.

`0x0000c37c` takes no event argument. It signals/waits through existing
OS primitives and can invoke additional scheduler handling; it neither
allocates an object nor supplies timing fields.

## Reuse and instrumentation boundary

`0x0001c3b6` is cancellation-related machinery, not a free primitive.
For event state `>= 10`, it declines to unlink an event still occupying
the pending slot; otherwise it removes/reclassifies the event and sets
state 2 before further active-event handling. Reclassification helper
`0x0000bab6` places the node on another list with queue state 3, changing
classification 2 to 1. Thus this path explicitly retains list ownership.
No synchronous guarantee
that all callbacks and scheduler references are gone is established.

Callback-table replacement must account for pointers retained by existing
events: registration copies the pointer, not the table. Restoring only
the global default table pointer at `0x00202fb8` does not repair those
objects. A telemetry clone and wrapper retained until controller reset
avoid this particular reclamation hazard. Safe custom periodic insertion
still requires timing/accessor contracts and a proven completion path;
this evidence is insufficient to recycle a standalone custom object.

## Separate software-timer candidate

A bounded trace from the timeout used by `0x0000c37c` resolves a separate
callback timer facility. Evidence is in
`work/ghidra/inspect_software_timer.txt`,
`work/ghidra/inspect_software_timer_dispatch.txt`, and
`work/ghidra/clean_software_timer_disassembly.txt`.
This supports a small inert periodic-callback experiment without creating
a fake LE event. It is not an ISO radio scheduler.

| Function | Statically observed contract |
| --- | --- |
| `0x000070b0(timer, callback, arg)` | Clears `0x20` bytes, writes callback at `+4`, argument at `+0x10`, flags `+8 = 2`. Does not allocate or arm. |
| `0x0000742c(timer, callback, arg, period)` | Calls the initializer, stores period at `+0x0c`, sets flags bits `0x3`. |
| `0x000073d4(timer, delay)` | Under saved interrupt masking, removes an existing occurrence, sets active flag `0x4`, computes `clock + uint32(delay)`, and inserts by deadline. |
| `0x00007408(timer)` | Under saved interrupt masking, removes the timer and clears active flag `0x4`; returns 0 when found, 1 when absent. |
| `0x00007370(timer)` | Searches the timer list for pointer identity; returns 1 when present, 0 otherwise. No local masking. |

Timer storage is caller-owned, word-aligned, and `0x20` bytes. Fields are
next pointer `+0x00`, callback `+0x04`, flags `+0x08`, period `+0x0c`,
callback argument `+0x10`, unused-by-this-trace word `+0x14`, and 64-bit
deadline at `+0x18/+0x1c`. The list head is `word(0x00202010)`, separate
from radio-event lists. Initializing an already-linked object would destroy
its next pointer; initialize only fresh or safely cancelled storage.

`0x000071ae` obtains the clock from a 64-bit software base at
`0x00202018/0x0020201c` plus the complemented hardware down-counter at
`0x0032a004`. `0x00006f7a` programs the earliest deadline through MMIO
`0x0032a020`, `0x0032a028`, and `0x0032a02c`. Clock units remain unresolved;
delay/period values must be described as timer ticks until calibrated.

Dispatch at `0x0000723e(now_low, now_high)` removes each due timer and
clears its active flag. If periodic flag `0x1` is set, it calculates
`steps = uint32(now_low - deadline_low) / period + 1`, adds
`uint32(steps * period)` to the old 64-bit deadline, sets active flag,
and requeues before invoking `callback(arg, lateness_low32)` at
`0x00007298`. Nonzero period is mandatory: division is unchecked.
Very long callback delays and 32-bit period arithmetic overflow are not
handled by this formula. Short-period counter telemetry avoids requiring
broader assumptions about that arithmetic.

The normal interrupt path is concrete: ROM vector word at `0x00000088`
contains `0x00000385` (external IRQ 18); handler `0x00000384` calls
`0x00007358`, which reads the clock and tail-calls timer dispatch.
Callbacks therefore can execute in interrupt context. They must remain
bounded and must not call the thread-only radio registration API.
An additional dispatch path from `0x000072b0` exists, so this is not a
claim that every callback invocation has the same entry context.

An existing periodic client at `0x00028e1e` initializes timer
`0x00205968` with callback `0x00026355`, argument `0x100`, and period
`0x1e848`. It adjusts flag `0x2` through `0x00007442`, then a later
path `0x00028e3c` tests membership and arms with the same delay.
The existing timeout client `0x0000d874` initializes the timer used by
`0x0000c37c`; its callback `0x0000d86a` signals an OS event. These are
separate examples of periodic and one-shot use.

This is static API evidence, not live validation. A first probe can use
resident timer storage plus a counter-only callback, a nonzero period,
explicit cancellation, and controller reset as final storage reclamation.
It would establish controllable controller-side periodic execution, while
ISO timing and radio-event ownership remain separate work.
