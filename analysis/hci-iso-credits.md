# Diagnostic HCI ISO credits

`hardware/iso_credits.S` supplies a 148-byte position-independent command lookup
and completion-event helper. This is an isolated, offline-tested component;
installation and RF completion accounting remain integration work. It does not
establish CIS/BIS, advertise controller features, or turn queue acceptance into
proof of transmission.

## Placement and command response

Place the blob at resident allocation+3072 in the validated 4096-byte reservation.
Native ingress starts at +2048 and must end before +3072. Entry offsets:

| Offset | Entry/data |
| --- | --- |
| 0 | `iso_credit_lookup`, Thumb hook for RAM word `0x203a34` |
| 36 | Prior lookup pointer; initialize to saved hook before installation |
| 40 | `iso_buffer_size_v2`, command handler |
| 88 | `iso_emit_completed(count)`, task-context helper |

The default prior pointer is stock `0x000d035d`. Restore the saved RAM hook
before releasing this image. Do not chain the experimental vendor producer
alongside native ingress without proving producer serialization.

Lookup matches OGF 8 / OCF `0x60`, opcode `0x2060` LE Read Buffer Size v2.
Descriptor has command length 3 and Command Complete event length 12. ROM
dispatcher checks the command parameter length is zero. The handler additionally
rejects nonzero length with status `0x12` and zero capacities.

Successful return fields are status 0, stock LE ACL packet length/count,
ISO packet length 30, and ISO count 4. Length 30 covers the 4-byte ISO SDU header
and 26-byte LC3 SDU; it excludes the 4-byte HCI ISO packet header. Four credits
match the queue's effective capacity. Stock ACL fields come from the existing
ROM handler, not invented constants.

## Completion ownership

Call `iso_emit_completed(count)` only from the controller task, with count 1..4,
after the corresponding host packets finish RF transmission or are explicitly
flushed. Copying a queued SDU into the radio buffer is not completion. Do not
count fixture-generated fallback frames: they consumed no host ISO packet.

The helper creates one Number Of Completed Packets event for handle `0x0042`:
`13 05 01 42 00 count_lo count_hi`. It returns 0 for invalid count or allocation
failure, with no event ownership transfer. Preserve the pending count and retry.
Return 1 means the ROM event path owns the allocation; retire those pending
credits exactly once and never free or retry that same event. Return 1 does not
prove the host has already read the event. Handle changes require changing both
this helper and native ingress; the default diagnostic handle is not a
negotiated stream.

Integration can use two independent cumulative u32 counters: RF ISR owns
`completed`, task owns `reported`. ISR increments completed only after a host
packet finishes or is flushed. Task snapshots completed with the corresponding
memory ordering, computes wrapping `completed - reported`, validates 1..4,
and calls the helper. On return 1 it advances reported by the snapshot delta.
New completions arriving during allocation/send remain pending. Queue metadata
offset 24 holds `completed`; offset 28 holds the radio's in-flight flag.
The native transport state holds `reported`, not queue metadata offset 28.
Radio consumer occupies resident+3328..4096. No counters are updated by this
isolated helper. Reset counters only
after removing producers/consumers and resolving old stream ownership.

## ROM evidence

Saved clean ROM SHA256:
`d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038`.
GNU Arm disassembly and exact byte anchors provide static evidence; no new REA
execution evidence or live credit observations are claimed.

- `0x51744`: stock LE Read Buffer Size handler reads u16 ACL length at
  `0x2025b1` and u8 count at `0x2025b9`, writes response+6 and response+8.
  ROM table entry at `0x877fc` points to Thumb `0x51745`.
- `0x41214`: nullable event allocator `(event_bytes, code, parameter_length)`.
  Adds 8 object bytes, calls `0x179dc`, returns null unchanged on failure.
  Success zeros the allocation, writes object kind `0x7041` at object+4,
  event code at object+8, parameter length at object+9.
- `0x412d6`: stock Number Of Completed Packets constructor uses event `0x13`,
  parameter length 5, one handle, u16 handle, u16 completed count. Our helper
  follows that layout but uses the nullable allocation path.
- `0x40e52`: event delivery takes the allocation pointer. Event `0x13` bypasses
  mask filtering at `0x40e72`. Delivery hooks, pending queues and transport own
  the object after this call. There is no boolean delivery result.
- `0x6d052`: command dispatcher frees the command through `0x17ada` if its
  free flag remains nonzero. Buffer-size handler leaves the flag untouched.
  Completion helper owns no command object and must not free submitted events.

## Check

```sh
python3 analysis/check-iso-credits.py --output work/iso-credits.bin
uv run --with unicorn python analysis/check-iso-credits.py --emulate --output work/iso-credits.bin
```

Checker builds at two RAM placements and verifies identical images, exact ROM
anchors, register/stack preservation, lookup passthrough, all 256 command
parameter lengths, stock ACL edge values, completion counts 1..4, rejection of
invalid counts, allocation failure/retry, event ownership and adjacent guards.
Emulation executes the actual ROM stock ACL handler and nullable event-wrapper
instructions; lower allocator/memzero/delivery calls are mocked. Live task
scheduling, RF completion accounting, allocator pressure and host receipt still
require hardware proof.
