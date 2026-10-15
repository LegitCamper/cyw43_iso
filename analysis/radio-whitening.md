# CYW43439 whitening control candidates

Static analysis of `work/cyw43439-btrom-clean.bin`, SHA-256
`d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038`.
Addresses below are controller addresses. No hardware execution was performed
for this analysis. Saved Ghidra output in `work/event-phase-radio/` and focused
GNU Arm objdump disassembly supply the evidence; no REA Evidence ID is claimed.

## Result

Do not identify `0x00318b6c` bit `0x200` as whitening-disable. Normal LE
connection code changes this bit with buffer-availability/control state.
Stronger whitening candidates are bits `0x300000` in the same register and
the logical-channel value written to `0x00318bf4`. Their exact hardware
semantics still require isolation on air; static evidence alone does not
establish whitening or separate TX/RX enables.

## Why bit 0x200 is insufficient

- `0x47578` writes baseline `0x01002140`, invokes `0x44450`, writes the
  connection access address and CRC init, then ORs `0x200` if RAM
  `0x207142` bit 3 is set (`0x4759e..0x475ae`).
- `0x475c6` obtains a buffer through `0x72262` when RAM `0x207138` is null.
  It sets bit 3 on allocation failure and clears it when a buffer exists.
  `0x72262` branches to `0x745d8`, which calls allocator `0x17c7c` with pool
  `0x208d14`, returning null or the allocation plus eight bytes.
- There is also a deliberate no-buffer path: `0x47900..0x47922` checks
  event `+0x8e == 0`, a configuration byte, and event `+0x142` bit 1;
  it calls `0x46e34` and sets bit 3. `0x46e34` passes any current buffer
  to `0x7202a`; `0x46e24` clears the pointer and sets bit 3.
- `0x44802..0x44852` retries allocation, then when bit 3 remains set
  changes header/control state, ORs `0x200` into `0x318b6c`, ORs
  `0x01000400` into `0x318364`, and ORs `0x80000` into `0x318b60`.
  `0x494fe..0x49524` clears these same three masks on the opposite path.
- DTM setup `0x71252` unconditionally writes `0x01002340` at
  `0x712a4..0x712a8`. This baseline differs by `0x200`, but DTM also
  changes other controls and omits normal setup helper `0x44450`.

Inference: buffer availability and receive/header flow control are a stronger
explanation for bit `0x200` than whitening. Exact function remains unknown.

## Channel-dependent control

`0x44450(channel)` reads RAM `0x203154` bit 0:

```text
bit 0 clear: 0x318b6c &= ~0x300000
bit 0 set:   0x318b6c |=  0x300000
             0x318bf4  = channel
```

The first reset scatter record at ROM `0x89e6c` copies `0x3590` bytes
from ROM `0x8bc8c` to RAM `0x200718`. Thus RAM `0x203154` initially
comes from ROM `0x8e6c8`, whose byte is `0x19`: bit 0 starts set. Later
configuration/patch writes were not exhaustively traced.

Normal setup at `0x4758a` passes event byte `+0x83` to this helper.
The channel-selection path `0x47194` computes hop/remap selection;
`0x47240` writes the resulting logical data channel to event `+0x83`.
Immediately afterward it stores the CYW43 RF offset in event `+0x7a`:

```text
offset_from_2402_MHz = 2 * channel + (channel <= 10 ? 2 : 4)
frequency_MHz = 2402 + offset_from_2402_MHz
```

The offset is relative to 2402 MHz, not 2400 MHz: data channel 0 yields
2404 MHz, channel 10 yields 2424 MHz, and channel 11 yields 2428 MHz.
DTM channel 0 instead writes offset 0 for 2402 MHz. Consequently `0x318bf4`
receives the logical channel, not its RF offset.
DTM omits this helper and leaves mask `0x300000` clear. Normal setup with
the default global and successful allocation therefore yields
`0x01302140`, not merely its intermediate baseline `0x01002140`.
This is consistent with channel-seeded BLE whitening. It is not proof:
the register could control another channel-dependent transformation.

## Reproduction and remaining experiment

Run `python3 analysis/check-radio-whitening.py`. The check verifies ROM
identity, scatter mapping, literal addresses, and relevant Thumb bytes;
it does not certify hardware behavior or runtime patch routing.

The hardware owner can distinguish candidates by holding AA, CRC, channel,
payload, and radio path constant while varying only `0x300000` and the
channel value, then comparing receiver raw bytes/CRC with whitening
enabled and disabled. Do not infer success from firmware accepting a
command or a local TX counter. Preserve independent control of `0x200`
until its buffer/header behavior is understood.
