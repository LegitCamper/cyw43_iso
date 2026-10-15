# Stock DTM recurrence and update boundary

Static evidence: `work/cyw43439-btrom-clean.bin` (SHA-256
`d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038`),
`work/btrom-clean-patchram.elf`, and saved disassembly/pseudocode
`work/event-phase-radio/test-{radio,payload,lifecycle,cleanup}.txt`.
No devices were accessed. Addresses are controller addresses. No REA
Evidence IDs or measured packet timestamps are claimed.

## Result

Stock DTM repeats through a scheduler countdown, not a 625-us busy-wait
inside the TX completion callback. The exact reload field is halfword
`0x2097c0` (parameters `+8`); the current countdown is word `0x203a7c`.
The callback at table `+0x24` decrements it and writes `0x318014` back to
itself when the countdown reaches zero. The firmware's length conversion
uses 625, supporting a nominal 625-us tick. Individual tick spacing, RF
start/end times, and jitter have not been measured.

Payload is copied into the radio window during phase zero only. Updating
the RAM pointer or its bytes afterward does not reload the radio window.
Stock code changes RF channel after TX completion, providing a useful
update precedent. No proven race-free payload-update boundary is established
by this static pass; completion followed by refill before the next explicit
rearm is the narrow candidate for a controller-resident experiment.

## Fields and callback roles

The singleton event is `0x209788`, parameters are `0x2097b8`, and callback
table pointer is word `0x203a80`, initially ROM `0x8b634`.

| Location | Width | Observed role |
| --- | --- | --- |
| event `+0x1c` | word | Parameter pointer |
| event `+0x20` | word | Nonzero diverts completion to asynchronous removal |
| parameters `+0x00` | word | RAM payload source for phase-zero copy |
| parameters `+0x04` | byte | Header/pattern argument to `0x444d6` |
| parameters `+0x05` | byte | Payload length |
| parameters `+0x06` | byte | Zero selects TX, one selects RX |
| parameters `+0x07` | byte | CYW43 RF offset from 2402 MHz |
| parameters `+0x08` | halfword | Countdown reload |
| parameters `+0x0a` | halfword | TX-completion count, wraps at 65536 |
| parameters `+0x14` | word | Bit 7 enables optional hopping; bits 13:8 hold its logical channel |
| `0x203a78` | word | Delay argument used by TX completion; reset value 20 |
| `0x203a7c` | word | Live countdown |
| table `+0x00` | pointer | `0x712f2`: dispatches radio status bits |
| table `+0x10` | pointer | `0x71546`: phase zero calls `0x71252`; other phases return |
| table `+0x18` | pointer | `0x7143e`: receive processing |
| table `+0x1c` | pointer | `0x714ce`: transmit completion processing |
| table `+0x24` | pointer | `0x713fc`: countdown and repeat trigger |

`0x711fe` initializes the event, clears the completion count, and initializes
the countdown to `0xff`. Phase setup later replaces it with parameters `+8`
at `0x712dc..0x712e0`.

## Cadence arithmetic and actual dispatch

HCI TX setup at `0x524a4..0x524aa` passes payload length to `0x7156e` and
stores the result in parameters `+8`. The exact function is:

```text
airtime_expression = 80 + 8 * payload_length
reload = floor((airtime_expression + 873) / 625)
       = ceil((airtime_expression + 249) / 625)
```

The 80+8*N expression matches uncoded BLE 1M packet airtime. The additional
249 is arithmetic equivalence, not a discovered hardware guard-time label.
Lengths 0..37 yield reload 1; 38..115 yield 2; 116..193 yield 3;
194..255 yield 4. These ranges describe the arithmetic, not proof that the
legacy HCI handler accepts every length in that range.

At `0x71420..0x7143a`, TX mode executes:

```text
countdown = countdown - 1           // 32-bit wrap
if signed(countdown) <= 0:
    MMIO[0x318014] = MMIO[0x318014]
    countdown = parameters.reload  // zero-extended halfword
```

The apparent self-write is a hardware action and cannot be deleted as a
redundant assignment. Its role as repeat trigger follows the surrounding
DTM logic; its precise edge-to-RF latency is unknown.

`0xdc5a` dispatches table `+0x24` for ready-list events in state 2. Both
`0x1bd8c` and `0x1beac` call it. ISR `0x16e18..0x16e3e` selects those
paths from pending bits `0x10` and `0x08`; other ROM callers also exist.
Thus the static graph does not by itself prove a callback exactly every
625 us or exclude extra callbacks under unusual scheduler transitions.

Existing `work/controller-tx-snapshot-probe/tx-instrumented.txt` records
402 instrumented callbacks and stock count 402. The host source requests
a 250-ms run, but HCI setup/readback adds uncontrolled boundary time.
That result is consistent with roughly 1600 packets/s; dividing 250 ms by
402 is not a valid measured packet interval.

## Completion and channel updates

`0x712f2` reads/acknowledges status through `0x16e96`; bit 4 dispatches
table `+0x1c`. Function `0x714ce` then:

1. Increments parameters `+0x0a`.
2. Calls `0x1711a(word(0x203a78))`. This branches through `0x71a6` to
   `0x7130`, a delay loop with timer and fallback-loop paths. Reset scatter
   initializes the argument to 20. This is a settle delay, not the repeat
   interval; its physical duration still depends on timer/calibration state.
3. Calls `0x602e(3)` and finite-test bookkeeping `0x75760`.
4. If event `+0x20` is nonzero, clears `0x318014` and requests removal via
   `0x713e4`/`0x1c97e`.
5. Otherwise, if parameters `+0x14` bit 7 is set, computes
   `channel = (channel + 5) % 37`, stores it in bits 13:8, and writes
   `0x3186a4 = 0x80 | (2*channel + (channel <= 10 ? 2 : 4))`.

That RF offset is relative to 2402 MHz: data channel 0 is 2404 MHz.
The completion counter marks callback invocation after a radio status event;
it records neither an RF completion timestamp nor a programmed deadline.
Stock hopping is fixed hop-by-five and is not BIS channel selection #2.

## Payload ownership and candidate safe point

Patched phase-zero path `0x712ec -> 0xd0200 -> 0xd03b8` builds the
header and calls `0x44ed2(0, header, parameters.payload, 0)`. That synchronously
copies to selector-zero window `0x370800` and commits the low half of header
register `0x318b68`. Neither `0x714ce` nor `0x713fc` reloads that payload.
See `iso-event-path.md` for the copy's padding requirement: an aligned
positive-length source must provide `max(40, align_up(length, 4))` readable
bytes. Merely supplying 31 readable bytes is insufficient.

The existing `hardware/controller_tx.S` wrapper redirects the pointer only
around phase zero, then restores it. Repeated transmissions therefore reuse
the hardware copy, which explains why pointer restoration does not restore
the transmitted payload.

A narrow follow-up can wrap TX completion, call stock completion first,
check that the event remains current/active with no removal pending, then
refill header/payload and apply the next channel before allowing another
repeat trigger. Alternatively, the countdown callback can refill immediately
before its trigger when countdown <= 1, but only after confirming the prior
TX completed. Both approaches need a bounded controller-side ready/busy
protocol and observed timing. Static call order does not prove FIFO idle,
prefetch completion, interrupt non-preemption, or an adequate refill budget.
Host-side HCI writes between individual packets cannot supply that guarantee.

## Single BIS at a nominal 10-ms interval

Ten milliseconds equals sixteen 625-us ticks. A constrained one-packet-per-
event experiment can use reload 16, with payload/channel updates once per
confirmed TX and an independently measured 10-ms start-to-start interval.
This avoids a 10-ms busy-wait inside the radio interrupt. It is a recurrence
candidate, not an ISO implementation or a proven absolute anchor.

For one BIS with BN=NSE=1, payload <= Max_PDU, and no encryption, this gives
a small path to testing cadence. Remaining requirements include BIS header
layout and payload counter, BIS access address and CRC initialization,
channel selection #2 and whitening, event-counter/anchor synchronization,
BIGInfo/periodic advertising, ISOAL/SDU timing, receiver validation, and safe
coexistence/lifecycle behavior. Existing DTM AA/CRC/header/hopping do not
satisfy those requirements.

Run `python3 analysis/check-dtm-timing.py` to verify the cited ROM bytes,
callback table, reset values, patch helper, and cadence arithmetic. The check
does not measure on-air timing or certify a safe update boundary.
