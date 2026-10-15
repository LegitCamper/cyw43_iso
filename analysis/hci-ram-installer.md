# Stock HCI RAM commands and Embassy installation

Stock `Write_RAM` (`0xfc4c`) and `Read_RAM` (`0xfc4d`) exist, but cannot be
assumed usable after protected startup. Both return status `0x12` without
touching the requested address when protection byte `0x200541` and
configuration-complete byte `0x200682` are nonzero. A public HCI-only installer
is therefore not a viable stock integration route. Use a narrow Embassy
backplane extension; keep its bus under the driver's existing ownership.

Evidence is the SHA-256-identified clean ROM
`d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038`.
No new hardware command was sent for this analysis.

## Dispatch and gating

ROM vendor lookup `0x11a96` indexes eight-byte descriptors at `0x86320`.
Patch lookup `0xd035c` overrides OCF `0x19`, `0x1a`, and `0x18b`, leaving these
commands on the ROM path.

| Command | Descriptor | Handler | Parameters | Response |
| --- | --- | --- | --- | --- |
| `fc4c` Write_RAM | `0x86580` | `0x12506` | address LE32, 0..251 data bytes | Command Complete, status |
| `fc4d` Read_RAM | `0x86588` | `0x1250e` | address LE32, length u8 | Command Complete, status, 0..251 bytes |
| `fc4e` Launch_RAM | `0x86590` | `0x12524` | boot/download dispatch | Not a general post-boot call primitive |

Write handler calls `0x7bfa`; read handler calls `0x7d70` and emits its locally
built event through `0x4117e`. Common completion builder `0x42182` writes event
`0e`, parameter length, command credit `01`, and opcode. Read status is at
event byte 5, data at byte 6. Maximum HCI event parameter length 255 leaves
251 read bytes. Read parameters must be exactly five bytes; writes require
at least four. Malformed/blocked cases return `0x12`.

Protection cases, directly exercised with real ROM Thumb under Unicorn:

| `200541` | `200682` | Result |
| --- | --- | --- |
| 0 | either | No address-window restriction in these handlers |
| nonzero | 0 | Address through final byte must lie in `209bac..220000` or `d0200..d8000`, inclusive comparisons |
| nonzero | nonzero | All reads/writes rejected before memory access |

These comparisons are implementation evidence, not a safe input-validation
contract: callers must still reject overflow, MMIO, unknown ownership and
zero-length corner cases themselves. Pre-complete allowed windows exclude
lookup pointer `0x203a34`, allocator state `0x200560`, and task context pointer
`0x201f1c` required by the runtime bootstrap.

ROM initialization `0x38b4` reads OTP through `0x35d8`; when bit 10 is set,
the same straight-line branch stores signature-validation enable `1` at
`0x200681` (`0x38dc`) and RAM-command protection `1` at `0x200541` (`0x38e6`).
Retained successful stock captures in `work/config-validation/stock.txt`
show validation enabled and configuration complete, both `1`. This establishes
the protected startup path. A direct post-startup read of `0x200541` was not
retained, and all possible later flag writes were not exhaustively excluded.
Do not claim a new live `fc4c/fc4d` result from this static evidence.

`fc4e` consults a download state bit and boot context callback `+0x108`;
it is not an unconditional branch to its address parameter. Entering download
mode via `fc2e` is not a substitute for an owned post-boot installation path.

## Memory access details

Ordinary writes call `0x7b02`. Partial first/last words use aligned 32-bit
read-modify-write; full words use aligned stores. Ordinary reads call
`0x7ce2`, which reads aligned 32-bit words even for unaligned addresses and
short requests. Both may touch bytes outside the logical requested range.
Addresses with top byte `ff` select separate storage routines, not SRAM.
Do not use these commands for MMIO probes.

The offline check verifies descriptor/literal bytes, rejection before target
access, allowed SRAM/patch writes in unprotected/pre-complete states, every
write alignment and tail, read limit, and parameter-length rejection:

```sh
uv run --with unicorn python analysis/check-hci-ram-installer.py
```

58 cases pass. Read emulation deliberately ends on word boundaries: Unicorn
incorrectly carries IT state beyond a skipped `BX` at `0x7d60` on the other
tail path. That emulator limitation is not evidence of a controller defect.

## Embassy seam

Embassy `cyw43` 0.7's public `BtDriver` implements `bt_hci::Transport` and
can send HCI commands and native ISO packets. Its `BtRunner`, backplane bus
operations, and Bluetooth firmware uploader are private. `Runner::run`
consumes the runner and never returns. No public post-boot RAM read/write
method exists in the inspected crate.

Implemented extension in `examples/embassy` adds checked Bluetooth-memory methods on an
exclusive `&mut Runner`, called after `new_with_bluetooth` returns and before
spawning `Runner::run`. Restrict addresses to pinned controller SRAM/patch
windows, enforce alignment, propagate bus failures, and read back writes.
For installation steps requiring HCI task execution, the extension also
services Bluetooth RX/TX while awaiting the bootstrap response;
otherwise an unstarted runner leaves queued HCI commands stalled.

Existing controller bootstrap already defines ownership and rollback:
validate stock identity and hook pointers, reserve resident SRAM through the
in-controller guarded allocator, verify code before publishing pointers,
and restore hooks on failure. Do not substitute a host cursor update for
that controller-side allocation or alter signed configuration. Full native
4096-byte installation sequence is ported in `examples/embassy/runtime.rs`,
with a Pico W executable in `examples/embassy-pico`. Host checks and firmware
builds alone do not establish successful live Embassy installation.
