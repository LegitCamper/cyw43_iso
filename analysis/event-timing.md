# Event timing and callback extension contract

Static analysis of `work/cyw43439-btrom-clean.bin`, SHA-256
`d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038`.
This pass performed no hardware execution. Evidence is retained in
`work/ghidra/inspect_clean_event_timing.txt`,
`work/ghidra/inspect_clean_event_timing2.txt`,
`work/ghidra/inspect_clean_event_timing3.txt`,
`work/ghidra/clean_scheduler_timing_disassembly.txt`, and
`work/ghidra/clean_clock_getters_disassembly.txt`. These are local
Ghidra/objdump artifacts, not REA Evidence records. Ghidra ran read-only;
the forced interior entries `0x4d8b8` and `0x1d3f0` are not valid function
starts and supply no conclusions below.

## Callback table +0x28 is an accessor

LE callback `0x00047556(event, selector)` returns these pointers:

| Selector | Return |
| --- | --- |
| 0 | `event + 0x28` |
| 6 | `0x00202fdc` if byte `event + 0xd8` is nonzero; otherwise null |
| 7 | `event + 0xac` |
| Other | null |

Registration at `0x0001c61e` requires a non-null accessor. That requirement
does not imply the accessor can return null for every selector: downstream
paths require selector 0's extension for timing operations. Selector 6 is
optional at `0x0000cba6`; a null result skips an additional timing check before
phase 1. Selector 7's full contract remains unresolved.

## Selector 0 extension

`0x0001d3f2(event, parameter)` obtains selector 0, preserves extension word
`+0x04`, clears all `0x28` bytes, restores that word, sets byte `+0x25` to 1,
writes `0x00005db6(0)` to halfword `+0x08`, and writes the caller's parameter
to halfword `+0x0a`. An optional global-dependent call to `0x0000b590` initializes
adaptive timing fields. If event byte `+0x15 == 1`, initialization also runs
`0x0000b7b8(event, extension, elapsed)` using a wrapped difference between
event word `+0x1c` and its selected clock.

Observed extension fields, relative to the returned pointer:

| Offset | Width | Proven behavior |
| --- | --- | --- |
| `+0x00` | word | Cleared by `0x1d3b2`; LE phase 2 tests nonzero before copying extension `+0x10` to event `+0x0c` |
| `+0x04` | word | Preserved across initialization; compared with current selected clock by `0xb7b8` |
| `+0x08` | halfword | Cached value from `0x5db6`; compared with a fresh value by `0xb7b8` |
| `+0x0a` | halfword | Caller-supplied value added to local value from `0x5db6` or `0x5da6` |
| `+0x0c` | halfword | Used against MMIO `0x3180f4` by `0xc470` |
| `+0x0e` | halfword | Low 16 bits of elapsed argument stored by `0xb7b8`; divisor in `0xc470` |
| `+0x10` | halfword | Copied to event `+0x0c` on a conditional LE phase-2 path |
| `+0x12` | halfword | Calculated signed correction or zero from `0xb7b8` |
| `+0x14` | halfword | Calculated widening/budget from products divided by `1600` |
| `+0x16` | halfword | Subtracted in correction calculation and then cleared by `0xc470` |
| `+0x18` | halfword | Initialized from global; adaptive sample threshold |
| `+0x1a` | halfword | Sample count, incremented and reset by `0xc470` |
| `+0x1c` | halfword | Maximum observed absolute correction, accumulated/reset |
| `+0x1e` | halfword | Adaptive bound, initialized to `0x5da6() + extension[+0x0a]` |
| `+0x20` | signed halfword | Signed correction estimate, incremented/decremented by `0xc470` |
| `+0x22`, `+0x23` | bytes | Counters tested/reset by `0xb7b8` |
| `+0x24` | byte | Cleared by `0xb590` |
| `+0x25` | byte | Set to 1 by `0x1d3f2`; cleared by `0x1d372` |
| `+0x26` | byte | Adaptive mode: 0 uses fallback, 1 starts adaptation, 2 uses correction |
| `+0x27` | byte | Flags: `0x04` changes update selection; `0x08` gates `0xc470`; low bits changed during queue/lifecycle transitions |

Drift, clock accuracy, and receive-window widening are inferred meanings,
supported by clock differences and division by 1600. Field access widths and
arithmetic are proven. This is not a generic periodic-timer object. In
particular, a fabricated zeroed extension can reach division by its zero
halfword `+0x0e` when mode/flags activate `0xc470`.

## Clock choices for telemetry

`0x0000a1d0()` consists of a read of MMIO word `0x00318088` and return. Its
instructions perform no stores. Sibling `0x0000a1da(out)` reads that same word
into `out[0]`, then `0x00318084` into `out[1]`. Existing scheduler paths call
the latter at `0x0000ca0e` and `0x0000d278`. No new peripheral access was tested;
absence of a store does not establish peripheral read side effects.

The native coarse clock uses 28-bit subtraction: `0x00009942(a,b)` returns
`(a-b) & 0x0fffffff`. Pair-difference routine `0x0000a1b0` calculates
`((delta28 * 625) >> 1) + fine_a - fine_b`, with 32-bit multiplication.
This supports 312.5-microsecond coarse ticks and microsecond fine values;
wall-clock calibration remains unmeasured.

`0x0000a1ec(selector)` is different: it writes the selector to `0x00318200`,
reads `0x00318088`, then returns `0x0031822c`. `0x0000a560(selector)` calls it
and shifts the result right by one. Consequently neither is a side-effect-free
time getter. `0x0000a0dc(a,b)` returns `(a-b) & 0x07ffffff`; selected-clock
deadlines use this 27-bit domain, consistent with 625-microsecond slots.
Native and selected clocks must not be compared without establishing their
relationship for the selected event.

LE phase 0 calls `0xa1ec(byte(event+0x0e))` at `0x491c0`, shifts right by one,
then compares the wrapped difference from `word(event+0x1c)` against 5.
Phase 2 calls `0xa560(byte(event+0x0e))` at `0x49242` and stores its result
at `event+0xa8`. Capturing those existing RAM fields avoids additional MMIO
access. An entry-only wrapper sees the previous stored phase-2 clock; reading
the freshly stored value requires observing after the stock callback returns.

## Insertion boundary

Normal `0x1c61e` registration rejects null event/table/accessor, nonzero
`0x1bfdc(event)`, nonzero `0x85f58()`, occupied pending pointer `0x2042cc`,
and event types `+0x13` equal to 0 or 4. The rejection path supplies no explicit
status return. `0xc5de` manages an intrusive node at event `+4`, ordered by
descending byte priority `+0x12`; it does not allocate an event or set a period.
`0xc37c` signals scheduler synchronization/event machinery and has no interval
argument.

No inert periodic LL-event API was established in this bounded trace.
A separate nonradio callback timer is now statically mapped in
[event-ownership.md](event-ownership.md#separate-software-timer-candidate):
`0x742c` initializes caller-owned periodic timer storage, `0x73d4` arms it,
and `0x7408` cancels it. IRQ 18 can dispatch its callbacks; a bounded
counter-only callback is the first experiment candidate. Timer units and
live behavior remain unvalidated. Existing-event phase observation is
supported by the known callback contract, while constructing a new radio
event still requires complete type-specific dispatch, timing, ownership,
and completion contracts.
