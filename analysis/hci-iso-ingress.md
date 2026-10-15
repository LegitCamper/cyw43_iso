# CYW43439 SDIO HCI ISO ingress

Static evidence: `work/cyw43439-btrom-clean.bin`, SHA256
`d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038`.
Addresses are Thumb instruction addresses, unless explicitly identified as RAM.
GNU Arm objdump and saved ROM bytes supply this evidence; no new REA provider
evidence IDs or live observations are claimed.

## Stock behavior

Pico SDK `cybt_shared_bus.c::cyw43_btbus_write` emits four-byte SDIO framing:
24-bit little-endian payload length, then transport type. The host retains
type 5. The controller parser at `0x5fa0e` reads this framing and dispatches at
`0x5faa4`: type 1 command, type 2 ACL, type 7 bounded vendor message. All other
types, including ISO 5, branch to `0x5fbb6`.

That path calls `0x5fa02`, which enqueues message kind `0xc002` via `0x5f9e8`.
The transport task handles this at `0x4418c`, calling `0x418be(0)`.
The latter allocates event code `0x10`, payload length 1, and stores error byte
zero. Thus stock type 5 is consumed and requests an HCI Hardware Error event;
it is not forwarded to the HCI command lookup or silently accepted as ISO.
Allocation and event-delivery failures remain possible, so static evidence does
not promise a visible host event on every dropped packet.

## Ring layout and parser ABI

Global block G is `0x2038cc`. G+0xc holds shared base; G+0x10 and G+0x14 hold
host-to-controller and controller-to-host ring sizes, initially 4096 each.
Offsets after both rings are producer IN, consumer OUT, reverse IN, reverse OUT
(four 32-bit words). G+0x18 is the arrival callback, initialized to Thumb
`0x4402b`. Shared base is published through controller MMIO `0x650168` (host
register `0x18000d68`). Address mapping helper `0x7baec` can translate addresses
depending on G+4 flags; identity mapping must not be assumed at runtime.

At parser dispatch, r8 is the 24-bit length, r1 type, r4 payload pointer, r7
mapped ring start, r10 inclusive mapped ring end, r9 unwrapped payload end,
and r6 wrapped overflow length (zero if contiguous). Caller supplies context
in r0. Common completion rounds consumed position upward to four bytes, wraps,
and publishes consumer OUT only after the copy or handled flag. Pending data
invokes G+0x18 again. Parser returns a normal allocated object or zero.

Commands become object kind `0x703f` with command at object+9. ACL becomes
`0x7082` with packet at object+8. Type 7 only accepts <=20 bytes into global
`0x207110`; it is not an ISO SDU path.

## RAM hook and its limit

RAM word `0x201f1c` holds the allocated transport context. Initializer
`0x4422a` stores Thumb `0x44105` at context+0x100. Transport thread `0x1481e`
loads this callback at `0x1483c` and `0x14878`, then calls
`callback(context, event_bits)`. This offers a RAM replacement in task context.

Stock callback `0x44104` drains the entire context+0xd8 message queue while
event bit 0 is set. Messages `0xc000` and `0xc001` call parser `0x5fa0e` at
`0x44160`; parser requeues when ring data remains. Consequently a wrapper that
only consumes leading ISO packets and then calls stock callback is unsafe:
mixed command/ISO input can expose later ISO packets to stock parser within
that same stock callback invocation. A real replacement must own the complete
drain loop and preserve outgoing messages, notifications, allocations and wake
semantics, or use a validated ROM parser-dispatch patch. Neither is implemented
by this static report.

Command lookup RAM hook `0x203a34` executes after command transport parsing and
cannot receive raw type 5. A vendor-command ingress through that hook can be a
smaller experimental transport, but must be labeled nonstandard and bounded by
the command parameter length; it does not establish standard HCI ISO support.

## Requirements for resident ISO queue

- Validate ring indices, available aligned bytes, bounded payload length, ISO
  header RFU/PB/TS and 14-bit length, SDU metadata and supported handle.
- Copy wrapped payload into owned bounded SRAM before publishing consumer OUT;
  never retain pointers into consumed shared ring storage.
- Preserve address mapping, producer notifications and task wake behavior.
- Backpressure on a full queue, or explicitly define drop/completion semantics;
  returning an ISO credit before buffer ownership ends is incorrect.
- Keep task producer/radio consumer synchronization explicit; radio payload
  copy padding and hardware refill timing remain separate requirements.

`python3 analysis/check-hci-iso-ingress.py` verifies selected static byte anchors.

## Offline-validated RAM task replacement

`hardware/transport_callback.S` and `hardware/transport_callback.c` now implement
the task replacement. This is an experimental transport bridge, not standard
CIG/BIG establishment or a completed LE Audio controller. Static report sections
above explain why intercepting only the arrival callback is insufficient.

G+0x18 has proven calls from interrupt handling (`0x5f858`, LR `0x5f85b`) and
post-consumption parser requeue (`0x5fc40`, LR `0x5fc43`). Both callbacks are
`void callback(void)`. Allocator retry messages `0xc001` originate separately
at `0x5fae4`/`0x5fb4c`; their parser calls need not pass the arrival callback.
Stock parser reads OUT at `0x5fa30`, IN at `0x5fa42`, then starts copying with
no whole-parser critical section. An interrupt callback advancing OUT between
those reads can leave the interrupted parser using a consumed pointer. The
replacement therefore leaves IRQ notification behavior intact and makes the
transport task the only ring consumer. It does not call stock parser when the
ring is empty, closing the race where ISO arrives after an empty precheck.

The position-independent blob is currently 852 bytes, entry offset 0, state
offset 8. State consists of ten u32 words:

| Offset | Value |
| --- | --- |
| 0 | Existing resident allocation pointer, initialized before installing hook |
| 4 | Diagnostic ISO handle, default `0x0042`; host must agree |
| 8 | Block reason: 0 none, 1 full/budget, 2 unsupported ISO, 3 incomplete frame, 4 corrupt state |
| 12 | Accepted ISO packets |
| 16 | Callback invocations with event bit 0 |
| 20 | Stock parser calls |
| 24 | Last accepted SDU sequence number |
| 28 | Ring inspections |
| 32 | Thumb credit emitter pointer; zero disables credit emission |
| 36 | RF completions already reported; written only by transport task |

Place at resident allocation+2048 in a validated 4096-byte reservation. Populate
state allocation at resident+2056, then atomically install Thumb entry
`resident+2049` at `(*(u32*)0x201f1c)+0x100`. Preserve the prior callback and
restore it before freeing/unmapping any resident memory. Installer and actual
device probe remain the hardware owner's responsibility.

Queue layout matches the experimental vendor producer: metadata at resident+1792,
head/tail indices 0..4, five 40-byte slots at resident+1824, effective capacity 4.
Only the first 26 bytes of a slot are written; 14-byte radio-copy padding remains
zero. Queue metadata offsets are head 0, tail 4, accepted 8, rejected 12,
consumed 16, underrun 20, RF-completed 24, inflight flag 28. Producer copies all payload bytes,
executes DMB, publishes head, then advances SDIO OUT. Radio consumer must finish
copying before publishing tail and wake transport when blocked ingress can resume.

Accepted type-5 packets have exactly 34 HCI payload bytes: configured handle,
PB=2, TS=0, all RFU clear, data length 30, sequence number, SDU length 26,
status=0, and 26 payload bytes. Unsupported packets remain at the ring head;
they are not handed to the stock Hardware Error path or consumed as valid data.
Full/incomplete input also retains OUT. This diagnostic stalls on unsupported
ISO until explicit recovery; production rejection, stream teardown and host
controller procedure support still need integration. No credit is issued for
queue acceptance; task credit service uses only the separate RF-completed count.

Stock message helpers remain unchanged: outgoing `0x7041`, `0x7043`, `0x7084`
go to `0x440aa` and retain its ownership; arrival/retry `0xc000`/`0xc001` inspect
ISO before each stock parser call; `0xc002` emits Hardware Error then invokes
the wake helper, `0xc003` invokes only wake, `0xc004` invokes state processing.
Other consumed queue messages are freed exactly as the original callback does.

Build and emulate without hardware:

```sh
uv run --with unicorn python analysis/check-transport-ingress.py --emulate --output work/transport-ingress
```

Checks execute actual Thumb instructions at two RAM addresses with ROM I/O
helpers mocked: relocation equality, register/stack preservation, wrap and
translated ring addresses, ISO/command mixtures, allocator retry, producer
arrival after empty inspection, full queue and explicit consumer wake, malformed
headers/status/handle, incomplete data, bad indices, padding/guard preservation,
outgoing/error/free message ownership, and actual stock-ROM callback trace
equivalence for non-ISO inputs. Credit checks include allocation failure/retry,
RF completion during emission, counter wrap, count bounds, and no duplicates
or credits before RF completion. Live equivalence of mocked ROM helper
contracts and timing remains to be checked by the hardware owner.

### Credit service and allocation-free ISR wake

Final 4096-byte allocation partitions are native callback +2048..3072,
`iso_credits.S` helper +3072..3220, radio consumer helper +3328..4096.
Set native state+32 (resident+2088) to Thumb `resident+3161`, the helper's
`iso_emit_completed` entry at credit-image+88. State+36 (resident+2092) is the
last reported cumulative RF-completed count. Metadata+24 is ISR-owned cumulative
completion; metadata+28 remains the radio's inflight flag, not a credit count.
Service snapshots completed, subtracts reported with u32 wrapping, emits only
counts 1..4, and publishes that snapshot after the emitter returns success.
New completions during emission therefore remain pending. Allocation failures
leave reported unchanged. Handle must be `0x0042` for this fixed-handle emitter.

Stock arrival helper `0x4402a` allocates eight bytes through `0x1797a`, whose
failure path can report/assert; it is unnecessary for a completion wake. Use the
existing event primitive directly from the radio callback:

```c
uint8_t *context = *(uint8_t * volatile *)0x201f1c;
if (context) ((void (*)(void *, uint32_t, uint32_t))0x9061)(context + 0xb4, 1, 0);
```

`0x9060` is the same event-OR operation reached by stock IRQ notification via
`0x1495a`. It saves/disables/restores PRIMASK around event state, allocates no
messages, and its scheduler tail `0x19e20` defers context switching while nesting
or scheduler-lock words `0x201fdc`/`0x201ffc` are nonzero. This is static evidence
for reusing the established ISR path; live callback/timing validation remains
the hardware owner's responsibility. Native callback now checks credits and
resumes blocked input even if the event wakes with an empty message queue.
Wake after publishing a completion, and retry on later radio callbacks while
completed differs from reported, including underruns; otherwise transient
event-allocation failure could leave the host permanently waiting for credits.

## Pico SDK raw host send

After BTstack reaches `HCI_STATE_WORKING`, use public
`cyw43_bluetooth_hci_write` from `cyw43.h`. It accepts a writable four-byte
SDIO prefix followed by HCI bytes, with packet type in prefix byte 3. The SDK
fills the three length bytes and synchronously copies into its shared ring.
Buffer must be aligned to four bytes and have capacity rounded up to four;
the supplied length excludes trailing alignment padding.

This isolated helper uses the diagnostic handle agreed above:

```c
static int send_iso_sdu(uint16_t sequence, const uint8_t sdu[26]) {
    uint32_t storage[10] = {0}; /* Four-byte alignment and 40-byte capacity. */
    uint8_t *packet = (uint8_t *)storage;
    packet[3] = 5;
    packet[4] = 0x42;
    packet[5] = 0x20; /* Complete PB=2, TS=0, handle 0x0042. */
    packet[6] = 30;
    packet[8] = (uint8_t)sequence;
    packet[9] = (uint8_t)(sequence >> 8);
    packet[10] = 26;
    memcpy(packet + 12, sdu, 26);
    cyw43_arch_lwip_begin();
    int result = cyw43_bluetooth_hci_write(packet, 38);
    cyw43_arch_lwip_end();
    return result;
}
```

The exact prefix after SDK framing is `22 00 00 05`; the following eight bytes
are `42 20 1e 00 sequence_lo sequence_hi 1a 00`. Total logical write is 38,
physical ring copy is 40, and the final two bytes stay zero. Passing 40 as the
logical length would incorrectly declare 36 HCI bytes. Stack storage remains
valid until the synchronous call returns; the driver retains no pointer.
`check-transport-ingress.py` independently compiles the repository's Rust
`IsoPacket::encode_into` and compares its complete framing to the emulator's
packet fixture, including sequence `0x1234` and padding.

Prefer this public write API over calling BTstack's transport `send_packet`
directly: the latter expects four writable bytes before its pointer and emits
an unsolicited `HCI_EVENT_TRANSPORT_PACKET_SENT` into BTstack bookkeeping.
This diagnostic helper sends no synthetic packet-sent event. Keep calls in
the existing host task/poll loop, not an IRQ, and preserve normal BTstack event
processing. Current probe already uses `cyw43_arch_lwip_begin/end` around shared
bus access, matching the outer serialization used by the SDK transport.

Important current-SDK limitation: `cyw43_btbus_write` discards the return from
`cybt_hci_write_buf` and returns zero. Ring-full asserts in debug builds and can
be silently dropped when assertions are disabled. A zero host API result is
therefore insufficient delivery evidence. Pace this bounded probe using
controller acceptance/OUT and eventual consumption/credit telemetry; do not
stream unbounded writes or reinterpret transport readiness as ISO credits.

Source locations in `/var/home/sawyer/projects/pico-sdk`:

- `lib/cyw43-driver/src/cyw43_ctrl.c:795`: public write / Bluetooth-up check.
- `src/rp2_common/pico_cyw43_driver/cybt_shared_bus/cybt_shared_bus.c:333`: SDIO framing and discarded lower-layer result.
- `src/rp2_common/pico_cyw43_driver/cybt_shared_bus/cybt_shared_bus.c:193`: aligned synchronous ring copy, bounds and producer publication.
- `src/rp2_common/pico_cyw43_driver/btstack_hci_transport_cyw43.c:114`: BTstack pre-buffer and synthetic packet-sent event.
