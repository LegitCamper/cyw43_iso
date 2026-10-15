# Pico W hardware results

Hardware: Raspberry Pi Pico W plus Raspberry Pi Debug Probe. Test date: 2026-10-13.

## Controller ROM and RAM captures

| Artifact | Size | SHA-256 | CRC32 | Status |
| --- | ---: | --- | --- | --- |
| `work/cyw43439-btrom-clean.bin` | 589824 | `d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038` | `91f45099` | Valid; three consecutive captures had the same CRC32, and a second decoded pass is byte-identical |
| `work/cyw43439-btrom.bin` | 851968 | `eacbd0114a542675a817f67598ba400c84e2194133b0237c1691421431ac5726` | N/A | Invalid oversized-read capture |
| `work/cyw43439-btram.bin` | 131072 | `99739dc37dac18b8440d7516817a1f8f96057f93693095e6ba3c084fccae5d85` | N/A | Invalid oversized-read capture |

The original probes requested 256-byte backplane reads, but Pico W SPI defines
`CYW43_BUS_MAX_BLOCK_SIZE` as 64 bytes. Release builds disabled the driver's
assert, so each read overflowed its internal SPI buffer. Per-line and full-image
CRCs only authenticated those corrupted host bytes. Repeated filler begins after
the first 64 bytes of many 256-byte records. Treat both artifacts, the 5390
candidate procedures, Evidence ID
`ev_fde7c76669c6ceaa8ae7b4072bd3ee95d23479bf6b7d3d1b03c988a3fe3ef1ab`,
and `analysis/rom-evidence.json` as invalid. The clean capture replaces them for
all scheduler analysis.

Corrected 64-byte reads are stable through `0x00090000`; the last non-`0xff`
ROM byte is at `0x0008f21b`. Reads above `0x00090000` intermittently return
unmapped-aperture data, so the ROM probe now stops at `0x00090000`.
`work/btrom_capture_64_final.txt` contains three complete matching passes.
`work/cyw43439-btrom-clean-pass2.bin` independently decodes to the same bytes as
`work/cyw43439-btrom-clean.bin`.

The Bluetooth core is reachable from RP2040 through CYW43 backplane address
`0x19000000 + controller_address`. No Nexmon Wi-Fi-core patch is needed.

## Stock Pico SDK firmware

Tested Pico SDK 2.2.0 firmware version:

`CYW4343A2_001.003.016.0065.0000_Generic_SDIO_37MHz_wlbga_BU_RPI_dl_signed`

Controller response:

- HCI version `0x0b` (Bluetooth 5.2)
- LMP version `0x0b`
- LMP subversion `0x2310`
- LE features `ff40010c00000000`
- all LE ISO feature bits clear
- all LE ISO supported-command bits clear

Direct zero-parameter commands `0x2060..0x2076` were also sent. Every ISO
opcode returned HCI status `0x01` (Unknown HCI Command). `0x206d` and `0x2074`
returned `0x12` because those are implemented, non-ISO commands with required
parameters. This rules out hidden ISO handlers masked only by capability bits.

## Newer patch test

Murata patch tested:

`CYW4343A2_001.003.016.0071.0017.1YN.hcd`

SHA-256: `429fb4766443c96e1ab96ac7eb65c2566349a3ee28d3af118645ed935e0b61c3`

Rust `import-hcd` converted it into Pico SDIO records. Full 1YN image failed to
open controller with error 180, consistent with board/config mismatch. Keeping
official RPI config and replacing only equal-sized Patchram code also failed to
reach HCI ready state. Stock `.0065` was restored and re-verified afterward.

## Hook-counter startup experiment

The `0x00074419` redirect image was flashed. It fails with
`Failed to open cyw43 hci controller: 180`; SDK value `180` (`0xb4`) is
`CYBT_ERR_HCI_INIT_FAILED`. This reports a controller initialization failure,
not an HCI command status or evidence that the wrapper executed.

The controls below used the same `picow_hook_counter` application. Earlier
observations are from terminal output in the investigation; only the guarded
redirect rerun has a saved complete result in `work/hook-controls/redirect.txt`.

| Controller image | Observation |
| --- | --- |
| SDK stock `.0065` | Earlier run reached the `HCI_STATE_WORKING` callback and printed `CYW43_HOOK_COUNTER advertising` |
| Unmodified `.0065` regenerated through `c-header` | Earlier capture showed no initialization error, but did not capture the working-state callback; a full successful startup was not established by that capture |
| Stock configuration plus appended wrapper, original hook table | Earlier run reached the working-state callback; telemetry became all zero. A separate read at `0x000d0f90` matched the appended wrapper's first 20 bytes |
| Same appended wrapper with hook redirected to `0x000d0f91` | Repeated startup failure `180`, including the guarded rerun |
| Stock image with config name `RPI` changed to `RPJ` | Earlier run failed with `180`; one byte changed at container offset `197`, with length and string terminator preserved |

The append-only and redirect containers differ only in the compact hook-table
entry inside the configuration at `0x0021f918`. The name-only mutation does not
redirect execution or append code. Together these observations support a
configuration-validation failure, but they do not establish whether it is a
signature, checksum, name-dependent behavior, or another startup constraint.
The `_dl_signed` version suffix is not proof of the rejection mechanism.
The subsequent ROM and live-RAM investigation below resolves this uncertainty.

Retained control containers:

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `work/sdk0065.bin` | 6970 | `d974384d0aa8124d30084f30d13519a3ed3b9f759f7ddd6a34ef3dcf1441bcf7` |
| `work/hook-controls/append-only.bin` | 7028 | `ee291892faca510186b37f208ee2a5a78cadeff869b7a683cb35594d8f9fd753` |
| `work/hook-controls/redirect.bin` | 7028 | `1ff6588f4fffc382c1866f343e6d9760924c409d7f084de88971c47e3d04ce4d` |
| `work/hook-controls/config-string.bin` | 6970 | `682071835e58de69be01193562525e8ed65320f2453c61a5ce32ecee42f9f33d` |

The original probe ignored `hci_power_control`'s return value and read telemetry
before HCI was working. Those startup/failure-time register values are not valid
hook evidence. The probe now returns on startup error and waits for
`HCI_STATE_WORKING` before reading telemetry. The guarded redirect rerun printed
`CYW43_HOOK_COUNTER_INIT_ERROR 180` without a `value=` record. Both custom and
stock probe builds succeed.

Recheck the controlled mutations and saved failure handling from the repo root:

```sh
python3 - <<'PY'
from pathlib import Path

controls = Path('work/hook-controls')
stock = Path('work/sdk0065.bin').read_bytes()
redirect = (controls / 'redirect.bin').read_bytes()
original_hook = bytes.fromhex('0d00194407010d06')
redirect_hook = bytes.fromhex('0d0019440701910f')
assert redirect.count(redirect_hook) == 1
assert redirect.replace(redirect_hook, original_hook) == (controls / 'append-only.bin').read_bytes()
expected = bytearray(stock)
assert expected[197] == ord('I')
expected[197] = ord('J')
assert bytes(expected) == (controls / 'config-string.bin').read_bytes()
capture = (controls / 'redirect.txt').read_text()
assert 'CYW43_HOOK_COUNTER_INIT_ERROR 180' in capture
assert 'CYW43_HOOK_COUNTER value=' not in capture
print('Hook control checks passed')
PY
```

Later append-only and config-name capture attempts stalled in `stty` inside the
host USB driver (`xhci_setup_device`), followed by disappearance of Pico and
debug probe devices. Their new text files contain only the TTY selection and
are incomplete, not successful or failed firmware tests. Capture processes were
stopped. A subsequent check found the debug probe again, but no Pico USB device.
This disconnect is not attributed to controller firmware by the available
evidence. No state-7 lifecycle correlation has been established.

After the debug probe reappeared, OpenOCD programmed
`work/hardware-sdk22/picow_hci_probe.elf` over SWD, reported `Verified OK`, and
reset the Pico. USB CDC returned. `work/hook-controls/stock-restored.txt`
contains repeated successful HCI replies with LMP subversion `0x2310`, LE
features `ff40010c00000000`, and `RESULT: controller ISO support absent`.
Programming output is retained in `work/hook-controls/restore-stock-openocd.txt`.
The board is therefore restored to working stock `.0065` firmware.
`work/custom-fw/cyw43_btfw_43439.h` and
`work/hardware-hook/picow_hook_counter.elf` are restored to the generated
redirect experiment with telemetry address `0x000d0fac`; they are not the image
currently running on the board.

## Configuration-validation gate resolved

ROM validation, fresh stock/name-only captures, and independent ECDSA P-256
verification identify signature rejection. The name-only control downloads
correctly, reaches the enabled validator with an invalid signature, and times
out waiting for firmware-ready. Its retained digest exactly matches offline
computation. Stock and append-only signatures verify; name-only and redirect
signatures do not. The signed range includes both the name and hook table.

See `analysis/config-validation.md` for exact ROM branches, the unusual SHA-256
compression/byte order, readback evidence, and a runnable verification check.
The new `picow_config_probe` uses 64-byte reads and distinguishes failure-time
diagnostic RAM from hook telemetry. No hook image was flashed in this pass.
Stock HCI probe was restored and verified again; programming output and two
successful live checks are retained under `work/config-validation/`.

## Feasibility boundary

Post-boot controller instrumentation is now proven. The command wrapper runs
and returns the unchanged stock HCI response; the state-7 wrapper runs during
advertising setup before connection. A matching uninstrumented control remains
at count zero. Both final lifecycle runs connect/disconnect and restore their
temporary changes. See `analysis/scheduler-hooks.md` for exact controls, timing
snapshot, limitations and capture hashes. Stock was restored again through
SWD with verification and two successful live HCI passes. ISO remains absent.

Infineon currently lists CYW43439 LE Audio support as **No** and its feature
availability matrix marks LE Isochronous Channels unsupported on CYW43439:

- <https://www.infineon.com/part/CYW43439>
- <https://community.infineon.com/t5/Knowledge-Base-Articles/AIROC-Bluetooth-feature-availability/ta-p/1107541>

This matches hardware results. Existing firmware is a Patchram overlay calling a
large mask-ROM controller implementation. A Rust rewrite of the small overlay is
possible, but cannot replace mask ROM in place. Actual LE Audio would require a
new controller scheduler, CIS/BIS link layer, ISOAL, HCI ISO transport, radio
timing, retransmission, and data-path implementation inside limited controller
RAM. Advertising ISO bits or adding HCI stubs would not produce an interoperable
radio link.

Embassy host ISO transport work remains gated on a controller that emits and
accepts HCI ISO packets. Practical hardware path is Pico W host plus external LE
Audio controller (for example nRF5340), not CYW43439 Patchram.

## Scheduler phase callback validated

A cloned stock event table now redirects only the phase callback to appended
controller code. Control counts remain zero; instrumented phases 0/1/2 each
reach 93, with 279 calls in the other-phase bucket. Both runs connect/disconnect
successfully with reason `0x13`, restore the global table pointer, and pass HCI.
Published table/wrapper storage remains resident until reset because embedded
events retain pointers. These are asynchronous occurrence counts, not radio
timing validation or ISO support. See `analysis/event-phase-probe.md` for exact
layout, captures, hashes, and runnable checks. Stock was restored through SWD
after both runs, with flash verification and two live HCI capability passes.
