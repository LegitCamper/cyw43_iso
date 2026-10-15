# Controller payload programming path

Static follow-up to `scheduler-hooks.md`; no hardware execution or ISO support
is established here. Input is `work/cyw43439-btrom-clean.bin`, SHA-256
`d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038`.
Local evidence is retained in `work/ghidra/inspect_clean_payload_programming.txt`,
`work/ghidra/inspect_clean_event_callback.txt`, and
`work/ghidra/clean_payload_copy_disassembly.txt`. These are Ghidra/objdump
artifacts, not REA Evidence or runtime traces.

## Proven writes

`0x00044ed2(selector, header, payload, flag)` programs two controller payload
windows. Four arguments are consumed; callers also place zero in a stack slot,
but this function never reads that slot.

| Selector | Payload destination | Original header mirror | Header MMIO field |
| --- | --- | --- | --- |
| 0 | `0x00370800` | halfword `0x0020713c` | low half of `0x00318b68` |
| nonzero (callers use 1) | `0x00370900` | halfword `0x0020713e` | high half of `0x00318b68` |

At `0x00044ee8`, the function clears mirror flag bit 0 or 1 in byte
`0x00207142`. At `0x00044f08` it extracts length using
`0x00044532(header, 0)`. If both length and payload pointer are nonzero,
`0x00044f1c` calls `0x00044676(destination, payload, length)`. When `flag` is
also nonzero, it then re-encodes `length + 4` through `0x000444d6`; the mirror
retains the original header. Finally `0x00044f44` writes the selected half of
`0x00318b68`, preserving the other half from the entry-time register read.

Length encoding depends on `word(0x00318714) & 3`:

| Mode | Decoded header bits (`0x00044532(header, 0)`) |
| --- | --- |
| 0 | bits 12:8 |
| 1 | bits 14:8 |
| 2 | bits 13:8 plus bit 15 mapped to length bit 6 |
| 3 | bits 15:8 |

Neither mode meaning nor current runtime mode is established. The four-byte
adjustment is consistent with the MIC arithmetic documented earlier; that
protocol label remains inferred.

## Copy contract matters

`0x00044676` is not an exact-length `memcpy`. For an unaligned source, it copies
whole words, rounding positive length up to four bytes. For an aligned source
it tail-calls `0x00086044`, which unconditionally copies 40 bytes before testing
remaining length, then rounds the remainder to four bytes. Thus a positive
aligned copy accesses `max(40, align_up(length, 4))` bytes. Any future custom
payload buffer must supply readable padding; supplying a pointer to an exact
short allocation is unsafe. This is a static access contract, not evidence of
extra over-air bytes.

## Queue and lifecycle boundary

`0x00044f4c(event)` obtains payload data via `0x00044a4a(queue, length, skip)`
from queue pointers at event `+0xdc` and `+0xe0`. The helper uses queue `+0x00`
as a linked-node pointer, `+0x08` as a byte cursor, and byte `+0x14` as a node
skip count. Each node has next pointer `+0x00`, data pointer `+0x04`, and length
in the upper half of word `+0x08`. A contiguous span returns its cursor;
a fragmented span is gathered through `0x00081f6e` into scratch pointed to by
`word(0x00202f60)`. Requested positive lengths are capped at 255. The helper
relies on queue consistency and does not validate arbitrary injected chains.

`0x00048da2` uses queue word `+0x0c` as remaining byte count when constructing
the next header. `0x000450fe` also gathers payload and invokes `0x00044ed2`;
its selector choice tests input status bits `0x20000` and `0x8000`.

`0x0004917e(event, phase)` reaches `0x00044f4c` when `phase == 1`: it first
calls `0x0000602e(6)`, clears event byte `+0x140` and halfword `+0x18c`, and
tail-calls payload preparation at `0x0004920e`. ROM word `0x0008af50` contains
the Thumb pointer `0x0004917f`; callback-table ownership and phase semantics
remain unresolved.

These functions program payload state for existing events. None establishes an
inert periodic-event insertion API. Required next evidence remains callback
registration/dispatch, scheduler ownership and timing, queue allocation and
completion rules, plus authenticated instrumentation. Calling the discovered
payload function alone neither creates an event nor implements CIS/BIS.

## Callback ownership and registration

Further static tracing resolves the callback table's owner. Evidence is in
`work/ghidra/inspect_clean_event_registration.txt` and
`work/ghidra/clean_event_registration_disassembly.txt`.

Reset scatter loading at `0x00000454` uses the table at `0x00089e6c`. Its first
record copies `0x3590` bytes from ROM `0x0008bc8c` to RAM `0x00200718`, using
copy routine `0x00000488`. Consequently ROM word `0x0008e52c`, containing
`0x0008af40`, initializes RAM word `0x00202fb8`. This is the callback-table
pointer at global `0x00202f9c + 0x1c`; it is not itself an event pointer.

`0x00047286(connection)` resolves the connection's event using byte
`connection + 0xe2` through `0x00046ede`, then clears `0x1a0` event bytes and
initializes its fields. It writes the connection pointer at event `+0x50`;
queue pointers at event `+0xdc` and `+0xe0` point to embedded blocks at
`event + 0xe4` and `event + 0xfc`. At `0x000473dc` it loads the callback-table
pointer and calls `0x0001c61e(event, table, mode)` at `0x000473e0`. Mode starts
at 2 and changes to 3 when connection byte `+0xe1 == 1` and global byte
`0x00202fa4` is nonzero.

The successful normal path of `0x0001c61e` stores the table pointer at event
`+0x00` (`0x0001c6a8`), writes 10 to event byte `+0x14`, applies mode through
`0x0001c330` to event `+0x16` bits 1:0, calls `0x0000c5de(event)`, stores the
event in a scheduler-global slot, and eventually tail-calls `0x0000c37c`.
There are earlier rejection conditions and an indirect override; this is not
an unconditional insertion primitive or a validated public API.

Callback table `0x0008af40` contains:

| Offset | Thumb pointer |
| --- | --- |
| `+0x00` | `0x0004794d` |
| `+0x04` | `0x000490c5` |
| `+0x08` | `0x000474f7` |
| `+0x0c` | null |
| `+0x10` | `0x0004917f` |
| `+0x14` | null |
| `+0x18` | `0x00048465` |
| `+0x1c` | `0x0004924d` |
| `+0x20` | `0x000487b3` |
| `+0x24` | `0x00048857` |
| `+0x28` | `0x00047557` |

Scheduler dispatch loads `event->table[+0x10]` and passes an explicit phase:

| Call site | Proven argument / ordering |
| --- | --- |
| `0x0000cf66` | Phase 0, after `0x0001abb6(event)` |
| `0x0000cc10` | Phase 1, on event in global `0x002042b8 + 0x08` |
| `0x0000c998` | Phase 2, on event in that same global slot |
| `0x0001d562` | Phase 1 for incoming event during `0x0001d4c2` event replacement |
| `0x0001d592` | Phase 2 for incoming event after writing 2 to its `+0x14` state byte |

The replacement path calls outgoing-event phases 4 and 5 around incoming-event
phase 1. `0x0004917e` handles phases 0, 1, and 2; phases 3 through 5 return
without work. Phase 1 is concretely payload preparation, while assigning
protocol names or precise time relationships to phases 0 and 2 still requires
runtime evidence. These phase values are separate from state 7 dispatched
through `0x0004ce02`; the static phase trace does not identify which host
operation triggers that other state.

This identifies a lifecycle-observation point and registration machinery.
It does not yet establish a safe inert event: object allocation/ownership,
registration preconditions, scheduling and completion fields, callback
`+0x28` contract, and controller timing must be resolved before constructing
a new event. Subsequent analysis resolves queue ownership, registration guards,
and the accessor's selector mapping in `analysis/event-ownership.md` and
`analysis/event-timing.md`. Live phase execution through a cloned table is now
validated in `analysis/event-phase-probe.md`. Independent event construction
and radio scheduling/completion remain unresolved; directly calling the LE
initializer would also initialize radio-related connection state.
