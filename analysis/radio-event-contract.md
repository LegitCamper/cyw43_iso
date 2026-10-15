# Compact scheduler event contract

Found a stock scheduler event with only `0x28` bytes and a null-returning
accessor. This removes the need to fabricate an LE connection or its timing
extension for an initial scheduler experiment. Its stock completion callback
changes controller clocks: do not call the stock constructor as an inert probe.
The initial static pass inserted no event. The later
[resident event probe](controller-event.md) validates one independent compact
event through registration, completion, cleanup and stock restoration. No ISO
or radio transmission was tested.

## Evidence and scope

Input: `work/cyw43439-btrom-clean.bin`, SHA-256
`d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038`.
Local Ghidra/objdump evidence lives under `work/radio-event-contract/`:
`constructors.txt`, `minimal-event.txt`, `minimal-lifecycle.txt`,
`dispatch.txt`, `lifecycle-cleanup.txt`, `class2-schedule.txt`,
`timing-path.txt`, and `rom-disassembly.txt`.

REA `binary_session` reported its Ghidra provider unconfigured because
`GHIDRA_INSTALL_DIR` was absent. Analysis instead used the existing
`/opt/ghidra/support/analyzeHeadless` and an isolated copy of the saved clean
Ghidra project, with `-readOnly -noanalysis`. These are local static artifacts,
not REA Evidence IDs. The original project, firmware and hardware were not
changed. Some Ghidra functions merge tail-called bodies; conclusions use exact
instruction addresses and ROM bytes. In particular, `0x1d100` is data, not a
function, and the containing-function result for that query is not evidence.

Run `python3 analysis/check-radio-event-contract.py` to verify ROM identity,
callback table, selected instruction anchors, call targets and cleanup switch.
The checker does not prove runtime behavior or absence of other references.

## Independent storage and constructor

`0x30f0c()` clears `0xc8` bytes starting at `0x205da8`: five resident objects
of `0x28` bytes each. `0x30f14(adjustment, correction, deadline, selector)`
addresses `0x205da8 + selector * 0x28` directly. No connection owner or heap
allocation is involved. The constructor does not clear its selected object,
validate selector bounds, or prove that its old registration has completed.
The pool initializer and cleanup callback supply the observed zeroing paths.
Do not borrow these stock objects for custom code.

| Object offset | Width | Constructor value |
| --- | --- | --- |
| `+0x0e` | byte | Selector argument |
| `+0x10` | byte | Type `0x30` |
| `+0x12` | byte | Priority `0x71` |
| `+0x13` | byte | Classification 2 |
| `+0x1c` | word | Clamped selected-clock deadline described below |
| `+0x20` | word | Original deadline argument |
| `+0x24` | halfword | Correction argument; completion reads it signed |
| `+0x26` | byte | Adjustment argument |

The event's remaining header is the shared intrusive-list scheduler header
already mapped in [event-ownership.md](event-ownership.md). Five slots imply
stock selectors 0 through 4; the generic timing code also uses a five-element
clock cache indexed by this byte without a local bounds check.

The constructor loads its callback pointer from RAM `0x202bbc`, whose reset
value is ROM `0x8aafc`, and tail-calls `0x1c61e(event, table, 3)` at `0x30f82`.
The scatter source for that initialized pointer is ROM `0x8e130`.
This call inherits all registration guards, hooks and blocking behavior already
documented for `0x1c61e`; it is not an interrupt-safe insertion primitive.

## Deadline arithmetic and optional extension

The constructor obtains `now = 0xa560(selector)` inside the existing lock and
interrupt-mask sequence. It computes
`earliest = ((now & ~1u) + 4) & 0x07ffffff`, then uses `0xa0e8(earliest, deadline)`
to choose the later value under the stock 27-bit wrapped comparison.
`0xa0e8(a,b)` returns zero for equality, 1 when
`0 < ((a-b) & 0x07ffffff) < 0x04000000`, and -1 otherwise.
An exact half-range difference is treated as -1, so this is not an unrestricted
ordering of arbitrary timestamps. Future probes must use near deadlines.
The minimum lead is three or four selected slots, depending on `now` parity.
The selected clock getter changes an MMIO selector; it is not a pure RAM read.

On registration-state processing, `0xc45a(event)` runs an optional table `+4`
callback, then enters `0xbd26(event)`. Classification 2 uses `event +0x1c`
directly. It calculates `delta = ((deadline << 1) - 0xa1ec(selector)) & 0x0fffffff`.

| Delta at this decision | Observed scheduling path |
| --- | --- |
| 6 through 13 | `0xb8f0`: state 1, queue state 2, near-deadline list `0x202310` |
| 14 through `0x08000000` | State 1, queue state 1, ordered list `0x202308` |
| Other values | `0xbab6`: ready list `0x202318`; state 3 or 4 notification, then state 2 |

The ready-list helper also changes classification 2 to 1. Classifications,
state values and queue-state values are separate fields. For the far list,
`0xb544` computes the wrapped distance and clamps values greater than
`0x04000000` to zero. Both it and `0xbd26` explicitly tolerate a null result
from accessor selector 0. Phase preparation also tolerates a null selector 6.

Thus the earlier LE timing-extension analysis is type-specific. A nonnull
accessor function is required, but a nonnull returned extension is not a
universal scheduler requirement. This compact stock event proves that useful
distinction without relying on a guessed extension layout.

## Complete compact callback table

ROM `0x8aafc` contains these nonzero Thumb pointers; all other entries through
`+0x28` are zero:

| Offset | Pointer | Observed callback behavior |
| --- | --- | --- |
| `+0x08` | `0x31139` | `0x31138(event)` clears exactly `0x28` bytes |
| `+0x10` | `0x3113f` | `0x3113e(event, phase)` calls `0x1aac0(event)` only for phase 0 |
| `+0x24` | `0x31149` | `0x31148(event)` performs the clock adjustment and queues removal |
| `+0x28` | `0x31207` | `0x31206(event, selector)` always returns null |

`0x1aac0` stores the event at `0x204cbc +0x0c`, clears several related scheduler
fields and may invoke existing hardware timing helpers. It is scheduler
integration, not an empty phase callback. Replacing phase 0 with a no-op is
not justified merely because this event never fills an LE payload queue.

`0xdc5a` walks the ready list at `0x202318`, saves its next cursor in
`0x2042b8 +0x20`, and invokes table `+0x24(event)` for objects in state 2.
The callback must check whether its object is actually current: stock
`0x31148` returns unless `0x1cf4e(event)` confirms equality with `word(0x2042b8)` and
`0x1cb1c(event)` confirms state 2. Being in the ready list alone is insufficient.

The stock callback then builds a clock adjustment from `+0x26` and signed
`+0x24`. Its nonzero-selector path calls `0x9c42`; selector zero calls
`0x9cfe(0, pair)`, `0x9c58(0)` and `0x1c27c(pair)`, with additional global
bookkeeping. For example, `0x9cfe` writes `0x318200`, `0x318204`,
`0x31820c` and `0x318208`. Therefore even a compact constructor with no
payload is not automatically inert. Omitting these clock-changing operations
requires a custom callback, not zero adjustment arguments passed to stock code.

## Removal and lifetime

The stock `+0x24` callback ends with `0x1d5b8(event)` followed by
`0x1c97e(event)`. The first clears `0x2042b8 +0x14` and signals the scheduler
waiter only when that pending pointer equals the event. It is acknowledgement,
not reclamation. The second requests asynchronous removal: state 14, then
`0xc5de` queueing, subject to its existing guards and override hook. It permits
interrupt context and does not itself wait for cleanup.

`0xdfa0` subsequently moves pending-state events out of the ready list only
after they differ from both current and incoming pointers
(`0x2042b8 +0x00` and `+0x08`). Processing at `0xde82` calls `0xc57e` to unlink,
then `0x321c8` to clear matching extra references at `0x202be0 +0x18/+0x1c`.
For state 14 its exact switch arm at `0xdf2c`:

1. Set state 8 and call optional table `+4` callback.
2. Set state 0, clear scheduler byte `0x2042b8 +0x3f`.
3. Call optional table `+8` cleanup callback.
4. Finish scheduler bookkeeping and acknowledge a matching pending pointer.

The compact table's cleanup clears its entire object, including the callback
pointer. This establishes a concrete terminal callback after normal unlinking.
It does not establish that custom storage may be freed from that callback:
the scheduler still retains its local event pointer for final bookkeeping,
hooks may replace paths, and this pass did not enumerate every retained
reference. Resident object/table/code until controller reset remains the
bounded lifetime rule. Registration return or pending-slot acknowledgement
alone must not mark storage reusable.

## Next implementation boundary

The smallest supported design candidate is a resident, one-shot `0x28`-byte
scheduler event with its own table. Preserve the compact phase callback,
null accessor and cleanup shape; replace `+0x24` with the current/state guards,
a counter, `0x1d5b8(event)` and `0x1c97e(event)`, omitting clock adjustment.
Keep a completion counter outside the object because cleanup zeros the header.
Use selector 0, a near future 27-bit deadline, no payload, and no reuse before
reset. Start with one object; periodic rearming and radio programming add no
value until this lifecycle is observed.

At the time of this static pass the candidate had not been published as executable
controller code. The subsequent [resident event probe](controller-event.md)
closes the one-shot registration and cleanup gates for its observed HCI thread.
The original gates were a proven thread-context trigger for registration, preserved type
`0x30` behavior outside the traced scheduler paths, and live confirmation that
the shortened completion path reaches cleanup and leaves stock HCI healthy.
The clock-adjustment constructor is evidence for the callback sequence, not a
proof that removing its middle has no hidden hardware prerequisite. Before
registration, preflight the default scheduler hook pointers and retained
storage bounds; registration can reject or block without a boolean result.

Radio-event construction remains a separate gap: this compact event does not
program access address, channel, packet header, payload, CRC, subevent spacing
or RX/TX completion. It is a better route to scheduler ownership validation,
not a CIS/BIS implementation. Static evidence here also supplies no physical
code-RAM upper bound for extending patch storage beyond `0xd1000`.

## Controller-owned RAM allocation

Further static evidence: `allocator.txt`, `allocator-pools.txt`,
`pool-setup-xrefs.txt`, `pool-initialization.txt`, `arena-reservation.txt`,
`allocation-failure.txt` and `free-and-arena.txt`, under the same work directory.
The checker also verifies allocator instruction anchors. No allocator was run.

| API | Proven normal-path contract |
| --- | --- |
| `0x17856(size)` | Resident arena reservation; rounds size up to 4, zeros storage, advances cursor under saved interrupt masking, returns pointer or zero |
| `0x179dc(size)` | Optional fixed-pool allocation; respects pool reserve counts, returns payload pointer or zero; does not clear payload |
| `0x1797a(size)` | Stronger allocation path used by stock packet constructor; failure additionally calls `0x5ed6`; avoid for optional probes |
| `0x17ada(payload)` | Pool release; finds owning registered pool by address range and returns its block to free list; null is ignored |

The resident arena is promising for code/event/table storage retained until
reset. ROM `0x4b08` initializes its cursor at `word(0x200564)` to `0x209bac`
and end at `word(0x200568)` to `0x220000`. These are boot defaults, not free
runtime addresses. `0x176f8` can advance the cursor from a configuration record;
pool initialization and many subsequent reservations also consume it.
`0x17856` compares `rounded_size < uint32(end - cursor)` strictly, then zeros
exactly that size and advances the cursor. An exact fit fails. Zero size returns
the current cursor without advancing it when space remains; it grants no storage.
Rounding overflow and reversed cursor/end are unchecked: reject zero and large
sizes and verify coherent, aligned live bounds before any custom caller.
Failure calls diagnostics `0x1017e(5,0x20)` and `0x9736`, then returns zero if
those return; this is not a silent allocation API. Zeroing happens with interrupts
masked, so a first allocation should remain small and occur outside radio work.

No free operation for arena reservations was established. In particular,
`0x17ada` is not their release function. The connection arena at `0x2035c0`
is itself reserved through `0x17856` by `0x5b85e`; its suballocator `0x5b81c`
is sized for specific stock connection pools and calls `0x418be(4)` on exhaustion.
Its unused-looking tail is not a general-purpose reservation.

For fixed pools, configuration starts at `0x201f28`, descriptor array at
`0x200574` has stride `0x24`, and linked-pool head is `word(0x200570)`.
ROM defaults decoded by initializer `0x177ee` are
`(payload size, count, reserve)` = `(16,32,3)`, `(48,36,2)`, `(96,20,2)`,
`(268,8,1)`. Configuration can change these; authoritative runtime capacities
are descriptor halfword `+4`, count halfword `+6`, backing base word `+8`,
free-list word `+0x0c`, free count byte `+0x10` and reserve byte `+0x11`.
Allocation returns `block +4`; preceding four bytes belong to allocator
metadata. Release does not validate block alignment or double frees, so pass
only the exact returned payload pointer, once, after all users release it.
An event/table may fit a 96-byte block; separate 268-byte code storage is only
a capacity possibility, not a proven executable allocation.

Stock `0x3122c` allocates a 32-byte object through `0x26c72`, fills callback
pointers at payload `+4/+8`, and queues it through `0x369fe`; its failure path
releases via `0x17ada`. Those callbacks target ROM `0x31227/0x31225`.
This proves that allocated RAM can own callback-bearing objects, not that its
payload bytes may execute. No MPU configuration or instruction-fetch alias
contract was resolved. Exact literal search for `0xe000ed90` through
`0xe000eda0` found no direct MPU-register constants; other address construction
remains possible, so that negative search proves no execution permission.

The existing `work/cyw43439-btram.bin` cannot supply live allocator state:
[hardware-results.md](hardware-results.md) explicitly marks it an invalid
oversized-read capture, and its cursor/end region contains repeated filler.
The corrected read below covers `0x200560..0x200603`,
`0x201f28..0x201f38` and `0x2035c0..0x2035cf`. Separate every backplane transfer
into at most 64 bytes. Current cursor/end plus pool descriptors determine
available ownership; no fixed config-tail address is reserved by this analysis.
Avoid crossing retained config ranges near `0x21e000/0x21f000` until their
post-boot lifetime is resolved. Allocation analysis alone does not establish
SRAM execution; the later bounded probe is in [sram-execution.md](sram-execution.md).

### Fresh stock snapshots and 512-byte guard

`work/controller-memory-probe/allocator-stock.txt`, SHA-256
`bab71e5f302f0fb1ec369f6ed219114289c3a8da6452a0b99daa5cb39749c706`,
contains two complete `CONFIG_BEGIN startup=0` snapshots with matching allocator
fields and controller-ready word `0x01000100`. These are two observations during
one run, not two independently restarted controllers. The reads were bounded
to 64 bytes per transfer. Validate with:

```sh
python3 analysis/check-radio-event-contract.py --capture work/controller-memory-probe/allocator-stock.txt
```

| Observed field | Value |
| --- | --- |
| Pool initialization byte `0x200560` | 1 |
| Resident arena cursor `0x200564` | `0x217e78` |
| Resident arena end `0x200568` | `0x220000` |
| Cursor-to-end span | 33,160 bytes (`0x8188`) |
| Cursor-to-static-config span | 24,968 bytes (`0x6188`), ending at `0x21e000` |
| Connection subarena base/end/cursor | `0x20ffac` / `0x212e20` / `0x212e20` |

The separate connection subarena is exhausted; it offers no spare capacity.
The four primary pools match ROM defaults and report all blocks free:

| Payload size | Count / free / reserve | Backing base | Minimum-free watermark |
| --- | --- | --- | --- |
| 16 | 32 / 32 / 3 | `0x20a1cc` | 31 |
| 48 | 36 / 36 / 2 | `0x20a44c` | 36 |
| 96 | 20 / 20 / 2 | `0x20ab9c` | 20 |
| 268 | 8 / 8 / 1 | `0x20b36c` | 8 |

Their free-list heads equal their backing bases. The fourth descriptor links
onward to `0x208ccc`; these four are not the complete linked-pool inventory.
No conclusion about every block or future allocation pressure follows from
these header snapshots. The resident span is ownership managed by `0x17856`,
not permission to write directly at the observed cursor.

A first runtime reservation can request exactly 512 bytes; the captured cursor
would produce `[0x217e78, 0x218078)`, well below retained configuration. A host
preflight alone is insufficient: another controller context may allocate
before the call. The controller wrapper must perform the following under one
saved interrupt-mask interval:

1. Reject nonzero IPSR before starting this initial probe; preserve the hook's
   registers, return value and ABI. Save PRIMASK and disable maskable interrupts.
2. Read initialization byte, cursor and end. Require initialization byte 1,
   a word-aligned cursor at least `0x209bac`, `cursor <= 0x21de00`,
   and `cursor + 512 < end <= 0x220000`. The cursor ceiling prevents overflow
   and keeps the allocation below `0x21e000`; strict comparison matches ROM.
3. Call Thumb entry `0x17857` with `r0 = 512`. Verify return equals the saved
   cursor and the new cursor equals that value plus 512 before publishing it.
4. Restore the original PRIMASK on every path. On any unexpected result, record
   failure and require reset; do not rewind the allocator or call pool release.

The mask helpers are exact: `0x45ee` is `mrs r0, PRIMASK; cpsid i; bx lr`,
and `0x45f6` is `msr PRIMASK, r0; bx lr`. Nested use by `0x17856` therefore
restores the already-masked state, allowing the wrapper to keep guard and
reservation atomic against maskable interrupts. NMI/HardFault behavior is not
covered. Keep this operation one-shot with an external result record; rejected
or repeated host commands must not consume another block.

For an aligned 512-byte block, `0x8216a` enters its aligned clear path and issues
32 four-register stores, writing 128 words before the allocator advances its
cursor. The entire clear occurs with interrupts masked. Actual duration, bus
stalls and controller clock rate were not measured; an instruction count is
not a microsecond bound. Perform the initial experiment without a live radio
connection. Preflight prevents the allocator's size-exhaustion path, which
calls `0x1017e` and formatted diagnostic wrapper `0x9736` before returning zero;
do not rely on diagnostic failure behavior as a recovery mechanism.

The snapshot alone establishes space for this guarded request. It does not establish
that an allocation occurred, that allocated SRAM permits instruction fetch,
or that a custom scheduler event can execute safely. The subsequent
[SRAM probe](sram-execution.md) validates one 512-byte allocation and execution
of a counter callback from that allocation, with stock HCI restored. Its
`IPSR == 0` guard passed, establishing thread context for that HCI lookup.
The later [resident event probe](controller-event.md) validates one blocking
registration from the observed HCI thread and its complete compact-event
lifecycle. Radio programming and reusable event ownership remain unvalidated.
