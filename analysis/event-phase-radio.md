# Live phase-1 radio register snapshot

`EVENT_PHASE_RADIO=ON` executes stock LE phase callback `0x4917e`, then records
registers `0x318714` and `0x318b68` only after phase 1. Live instrumented and
control runs pass connection, disconnection, callback-table restoration and
stock HCI checks. Both runs finish with SWD stock restoration and two capability
passes. This observes stock BLE radio setup; it does not transmit custom ISO.

## Implementation

The existing 112-byte patch reservation holds a 48-byte wrapper, 20-byte trace
area and 44-byte callback-table clone. Only the first two trace words are used.
`hardware/event_phase_radio.S` preserves callee-saved registers, stock return
registers and APSR, and keeps the stack aligned across the stock call. No extra
MMIO writes occur. The trace starts with `UINT32_MAX` as an unobserved mode.
Control publishes the same table clone with the original phase callback.

The global table pointer is restored after disconnection and an HCI barrier.
Existing events may retain the clone, so wrapper/table storage remains resident
until controller reset. RAM words are sampled asynchronously: a pair is not
guaranteed coherent, and skipped/overwritten phases are not counted. Host times
do not establish controller timing. Instrumentation adds execution time.

## Live evidence

Artifacts under `work/event-phase-radio/`:

| Capture | SHA-256 |
| --- | --- |
| `radio-instrumented.txt` | `2133cc050fb812a34253b6b2e31928647157667a850efc81d56fb2b9d67f52b3` |
| `radio-control.txt` | `9327562c8e997de489ed3340197d4e53a1fd665b3a23c221faac3ef850ee0432` |
| Both `*-stock-restored.txt` | `4eab5268f1f73c24986328ba61a2d091bf308a0ce7cf755a1969d98dd8b1e78d` |

Every recorded mode is `3`. Therefore the previously mapped length helper uses
header bits 15:8 for the observed phase-1 packets. Recorded TX header register
values include `0x00090613`, `0x09030019`, `0x000d0907`, `0x00010009` and
`0x000d0005`. The low/high halfwords correspond to windows `0x370800` and
`0x370900`. For example `0x00090613` represents header lengths 6 and 0;
`0x09030019` represents lengths 0 and 9. This establishes register contents,
not over-air transmission or successful peer reception of each sampled packet.

## Reproduction

```sh
python3 analysis/check-runtime-probe.py --prepare --phase
PICO_SDK_PATH=/var/home/sawyer/projects/pico-sdk cmake -S hardware -B work/hardware-phase-radio \
  -DCYW43_CUSTOM_BTFW_DIR="$PWD/work/event-phase-probe/custom-fw" \
  -DEVENT_PHASE_RADIO=ON -DSTATE7_CONTROL=OFF
PICO_SDK_PATH=/var/home/sawyer/projects/pico-sdk cmake --build work/hardware-phase-radio --target picow_runtime_probe -j4
python3 analysis/check-phase-radio.py --capture work/event-phase-radio/radio-instrumented.txt
python3 analysis/check-phase-radio.py --capture work/event-phase-radio/radio-control.txt
```

Control uses `work/hardware-phase-radio-control` with `STATE7_CONTROL=ON`.
Retained `run.py` flashes the selected experiment, connects the existing local
BLE peer, captures bounded output and restores stock in `finally`. It rejects
existing capture names. Source/build identities are in `source-and-elf.sha256`.

## Follow-up radio contract

Static artifacts `radio-setup.txt`, `radio-config.txt`, `radio-channel.txt`,
`test-radio.txt` and `access-address-disassembly.txt` use clean ROM SHA-256
`d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038`.
REA reports Ghidra unconfigured (`GHIDRA_INSTALL_DIR` unset); these artifacts
come from the existing local Ghidra project opened read-only with no analysis
and focused objdump output. No REA Evidence IDs or native session are claimed.

`0x47578(event)` programs LE connection radio state:

| Write | Source |
| --- | --- |
| `0x318b6c` | `0x01002140`, optionally OR `0x200` |
| `0x318b70` | word at connection `+0x34` |
| `0x318ba4` | word at connection `+0x00`, masked to 24 bits |
| `0x318b60` | zero |

Connection pointer is event `+0x50`. The same register pair is programmed by
standalone test-radio setup `0x71252` from globals `0x20315c` and `0x203158`.
Their scatter initializers are ROM `0x8e6d0 = 0x71764129` and
`0x8e6cc = 0x00555555`: standard LE Direct Test Mode access address and CRC
initial value. This corroborates access-address register `0x318b70` and CRC
initial-value register `0x318ba4`; arbitrary values have not been tested live.

Test-radio setup writes parameter byte `+7 | 0x80` to `0x3186a4`. Normal LE
phase-0 setup writes event byte `+0x7a | 0x80` to the same register. A test
channel update at `0x71528` maps data channel `c` to `2*c+2` for `c <= 10`
or `2*c+4` otherwise, matching MHz offsets from 2402 MHz for BLE data channels.
This is a frequency-control contract; PHY, whitening, hardware trigger and
completion semantics still require complete tracing and live validation.

The helper `0x44450(event[0x83])` does **not** establish channel selection.
Depending on `0x203154 & 1`, it updates control bits `0x300000` in `0x318b6c`
and optionally writes `0x318bf4`; its protocol meaning remains unresolved.
