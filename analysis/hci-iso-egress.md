# Constrained controller-to-host ISO transport

`hardware/iso_rx_emit.S` prepares one complete, timestamp-free, valid 26-byte
SDU for handle `0x0042`. The one-shot RX diagnostic now uses it to deliver
received Nordic markers over native HCI ISO. It is not installed by the TX
blob. Synchronization, ISOAL ordering and continuous reception remain separate
requirements; live captures and restoration are in `controller-rx.md`.

## ROM transport contract

Evidence: GNU Arm disassembly of clean ROM SHA-256
`d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038`
and `.0065` Patchram segment `0xd0200..0xd0f90`, SHA-256
`d9a12cb202ca648236e494b327e2d00720ade5507395a1a7fb7840db6ad586d5`.
These are static observations, not REA runtime evidence.

- `0x5fd3c` accepts outgoing object kinds `0x7041`, `0x7084`, `0x7043`,
  converting them to transport types 4, 2, 7. It cannot emit ISO objects.
- Its lower writer `0x5fc54(type, data, length)` accepts a raw type byte.
  It synchronously copies into the shared controller-to-host ring, returns 0
  on success and 1 when space is insufficient. It retains no source pointer
  and neither allocates nor frees the caller's buffer.
- Global `0x2038cc` holds shared base at `+12`, host-to-controller size at
  `+16`, controller-to-host size at `+20`. Reverse producer/consumer words
  follow both rings at `+8/+12`. Every shared pointer uses mapper `0x7baec`.
- `.0065` branch at `0x5fcb4` enters `0xd02ac`, which requires `length+8`
  free bytes. For the 34-byte HCI packet, aligned free space must be at least
  44 bytes even though the ring advances by only 40 bytes. Removing this
  margin could make a full ring indistinguishable from an empty one.
- Writer publishes the aligned reverse producer only after copying. Caller
  then invokes `0x5f87a`, which ORs bit 1 into register `0x702044` to notify
  the host. Stock outgoing dispatcher follows this same success-only order.
- Writer calls `0x5f940` for controller power/clock readiness. That routine
  can wait for hardware. The wrapper must run in the existing transport task,
  never the radio ISR. This does not provide a hard task-execution deadline.

## Emitter ABI and ownership

`iso_rx_emit(sequence, payload, length)` takes a zero-extended u16 sequence,
pointer to 26 readable bytes of owned controller SRAM and length 26. It rejects
interrupt context, out-of-range sequences, lengths other than 26 and pointers
whose complete span lies outside `0x200000..0x21ffff`. Bounds do not establish
allocation ownership: the caller must own stable storage throughout the call.

Return values are 0 (copied and host notified), 1 (ring full; input remains
pending) and 2 (invalid call; no transport operation). Sequence numbers and
queue ownership advance only on return 0. A successful call does not prove
the host has read the packet. Repeating a successful call duplicates it.

Wire bytes are:

```text
22 00 00 05  42 20 1e 00  sequence_lo sequence_hi 1a 00  [26 payload bytes]
```

First four bytes are the SDIO envelope, excluded from HCI length 34. Handle
flags `0x2042` encode PB Complete=2, TS=0. HCI ISO data length is 30,
including sequence and SDU-length fields. The host-facing H4 packet has 35
bytes including indicator 5. Physical ring consumption is 40 bytes.

No loss/status synthesis belongs in this fixed emitter. A future synchronized
receiver must supply event-derived sequence numbers, distinguish padding from
loss, preserve order and apply flush deadlines. The current DTM snapshot has
none of those timing guarantees.

## Intended task integration

Radio callback copies bounded payload/metadata into owned storage and publishes
completion with a memory barrier. Task observes publication before reading,
calls the emitter and retires the slot only on success. Ring-full return keeps
the slot for a later transport wake. All controller-to-host writers must stay
serialized in that task; do not call this from a host callback that can race
the stock outgoing queue. Receiver teardown must remove the producer before
retiring resident storage. Reset/reload remains the reclamation boundary.

## Executable check

```sh
uv run --with unicorn python analysis/check-iso-rx-emit.py --output work/iso-rx-emit.bin
```

The checker links identical images at two SRAM addresses and executes 140
cases against the real ROM writer, `.0065` writer patch, pointer mapper,
memory copy and host notification. Only power/clock readiness is stubbed.
Checks cover contiguous/wrapped output, translated shared addresses, sequence
boundaries, unaligned input, ring-full retry, no source mutation, exact ring
bytes including untouched padding, publish-before-notify, ABI and rejection
without transport access. Live power readiness, task scheduling and host
reception are not established by these offline tests.

Unicorn 2.1.4's global memory-write hook changes condition-flag behavior in
this ROM's copy tail: an isolated six-byte copy writes seven bytes with that
hook, while the unhooked copy writes six. The checker hooks only the producer
and notification words, leaving ROM copy execution uninstrumented, and checks
the whole ring afterwards. No firmware change or weakened padding comparison
was used to accommodate that emulator issue.

## One-shot task bridge and export

`hardware/iso_rx_task.S` wraps the guarded stock transport callback `0x44105`
at `(*(u32*)0x201f1c)+0x100`. It calls stock first, then services a completed
snapshot on task event bit 0. Snapshot, mode, header and status checks prevent
unrelated records from being emitted. The current bridge also requires the
stock bit-2 counter to advance exactly once, wrapping at 65536, with both other
counters unchanged. This checks consistency; it is not an independent CRC
definition. A separate delivered flag prevents duplicate delivery, while a
full ring leaves the snapshot pending. Sequence 0 is diagnostic metadata.
The radio hook claims the first bit-2 packet, including an unsupported length.
If that first packet fails the task guard, later packets cannot replace it;
the diagnostic must time out and reset. This is not a general receive filter.

The probe wakes the task using host version commands every 250 ms. No bounded
RF-to-host latency or autonomous continuous receive behavior follows. Both
callback pointers are restored after DTM stops; allocations remain until reset.

```sh
uv run --with unicorn python analysis/check-controller-rx.py --bis --iso --emulate --output-blob work/iso-blob-rx
```

Export contains `native.bin` (1024 bytes), `allocator.bin` (112 bytes, 1024-byte
reservation) and `manifest.json` with addresses, offsets, limitations and hashes.
The RX allocation is distinct from the 4096-byte TX image. The immutable snapshot
stays at offset 320, task wrapper at 576, delivery state at 736, phase trace at
768 and emitter at 800. The host initializes callback table entries before
publishing either hook. Embassy references these sibling artifacts directly.
