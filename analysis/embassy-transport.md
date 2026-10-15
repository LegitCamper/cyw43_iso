# Embassy SPI integration evidence

The controller blob has independent stock Nordic SDC reception evidence through
the Pico SDK host; see [controller-native-iso.md](controller-native-iso.md).
Embassy installation and the production runner now pass live tests. The
`rp2040_audio` application sends and receives credits for 1,000 SDUs; independent
stock Nordic SDC discovers the broadcast and receives 950 exact SDUs, including
recovery across a 250-ms refill pause. Both devices are restored. Complete
counts, limits and artifacts are in
[controller-native-iso.md](controller-native-iso.md#embassy-production-runner).

Passing diagnostic: `work/embassy-native-iso-window-reset-probe`, ELF SHA-256
`83c7b8e20a0c1a51048d86c621e125d32c1df3984f95be327722cdfb500b0ae9`.
Sent, accepted, consumed, RF-completed, reported and host-credit totals all
equal 20. It records four underruns, an empty queue, no in-flight packet and
no blocked input. Hook restoration and two external stock capability checks
pass. The diagnostic uses its startup Bluetooth pump, not `Runner::run`.

The smaller variant also passes the same checks:
`work/embassy-native-iso-minimal-transport-probe`, ELF SHA-256
`39bca1b14adc8f47ea6bc311407802be42a04213f4e4c6d50c698322cb38a94c`.
It restores original little-endian mode, PIO instructions and transfer timing.
It retains prearmed receive DMA, 16-byte F1 padding, flagged bulk access,
CHIPCOMMON window restoration and controller-memory chunks up to 64 bytes.
It does not retain the endian swap, explicit CS delay, FIFO restarts or added
completion fences. Native driver integration uses these transport changes and
also retains compiler fences after RX DMA completion. Its independently received
1,000-SDU test uses ELF SHA-256
`6e9352a789f1e117651bd7d0f15a1a521d0c725c826d529f9b77f1964dd0dd98`.

The native driver lives on branch `cyw43-iso` in sibling `embassy-cyw43-iso`.
It directly includes this repository's exported blobs. `rp2040_audio` owns the
normal-runner application. `examples/embassy-pico` remains an isolated diagnostic
using a generated crates.io driver copy; experimental changes there are not
automatically promoted to the native branch.

## Established requirements

- Raw version identifies Infineon `0x0131`, subversion `0x2310`. Pico SDK BTstack
  rewrites the manufacturer to Broadcom `0x000f`; Embassy does not.
- The tested boot image reserves 112 zero bytes at controller `0x000d0f90`.
  Its SHA-256 is recorded in the exported manifest. Unknown scratch is never
  cleared to force installation through a guard.
- F1 response padding is 16 bytes in the proven C path. Scalar and bulk reads
  must agree with the configured delay and buffer sizes.
- The controller aperture uses the GSPI 32-bit access flag `0x8000` in the bus
  offset, not in the absolute address or selected backplane window.
- Exact readback, stock-pointer checks and ring validation remain mandatory.
  Failure requires chip restart and firmware reload; HCI Reset alone does not
  reclaim resident allocations.

## Retained trials

Each directory is under `work/` and retains the capture, source/ELF hashes and
stock-restoration evidence. These are separate variants; results must not be
combined into a claim that a single variant passed every step.

| Trial directory | Change / observed result |
| --- | --- |
| `embassy-native-iso-read-matrix-probe` | Original PIO, divider 2, pad 16, flagged bulk reads. 300 rows / 900 scalar and bulk transactions match all asserted ROM/stock words. No installer writes or HCI traffic. |
| `embassy-native-iso-flagged-ring-probe` | Same read framing in normal diagnostic. Reset/version and stock guards pass. First allocator word reads `0x53ffe92c`, expected `0x53ffe92d`; delayed read agrees with mismatch. |
| `embassy-native-iso-flagged-bulk-probe` | Flagged writes and readback in chunks up to 64 bytes. First 64 allocator bytes verify. Next 48-byte chunk differs at its first byte: `0x21`, expected `0x20`; remaining bytes match. |
| `embassy-native-iso-mismatch-shape-probe` | Adds scalar, multiple-length and shifted-burst diagnostics without accepting retries. Fails before patch writes: stock task callback reads `0x00044104`, expected `0x00044105`. This RAM value alone cannot distinguish a bad read from prior boot-upload corruption. |
| `embassy-native-iso-mismatch-shape-repeat-probe` | Exact same ELF, repeated once. Fails during WLAN core initialization, before reaching planned diagnostic reads. |
| `embassy-native-iso-endian-probe` | Changes only bus byte order/DMA byte swapping relative to prior diagnostic; original PIO program, status, pad 16 and divider 2 retained. Chip initialization and shared base `0x0006861c` succeed; first HCI Reset times out. |
| `embassy-native-iso-endian-ring-probe` | Adds command-ring/control readback. Reset succeeds; version times out. Command bytes read back exactly, but controller leaves the second command unconsumed. Host-control bit 24 changes unexpectedly during read/modify/write. |
| `embassy-native-iso-rx-prearmed-probe` | Arms receive DMA before CPU-enqueuing the single read command; retains endian candidate settings. Allocator executes and image upload advances. One 64-byte readback returns stale-looking data beginning `0x01000100`; immediate scalar, bulk and shifted reads match expected image words. No instrumented RXSTALL warning. Exact verification still rejects installation. |
| `embassy-native-iso-cs-gap-probe` | Adds at least 100 ns after CS rises. Upload advances farther, then an expected zero block at `0x192184f8` reads back `0x01000100` followed by zeros; subsequent reads match zeros. No RXSTALL warning. |
| `embassy-native-iso-window-reset-probe` | Resets the backplane window to CHIPCOMMON after each bulk read/write, matching the C memory helpers. Retains preceding endian/prearmed-DMA/PIO-reset/settling/fence changes. Full diagnostic passes; this establishes the combination, not that every retained change is necessary. |
| `embassy-native-iso-minimal-transport-probe` | Original byte order and PIO timing restored; prearmed receive DMA plus corrected backplane accesses retained. Same full 20-SDU diagnostic and restoration checks pass. |

Pico stock firmware restoration and two stock capability checks pass after each
completed trial listed here. No Nordic firmware changes were needed for these
transport-only tests.

## Investigation boundary

Observed single-bit differences can go in either direction. Neither a masking
workaround nor accepting a matching retry establishes correct transport. The
planned shifted read puts the disputed word at a different burst position to
separate a read-boundary fault from stored corruption; neither attempted run
reached that diagnostic. Earlier modified-PIO trials also misread immutable ROM,
but that does not establish the cause of failures with the original PIO.

The C host uses big-endian bus mode with DMA byte swapping, no appended SPI
status, FIFO/state-machine reset per transfer, and a TX-only completion path.
Embassy starts with little-endian bus mode, no DMA byte swapping and appended
status. The passing diagnostic retains appended status; it does not require
the earlier speculative TX-only PIO program.

One specific timing hypothesis is RX FIFO exhaustion: the original read path
awaits transmit DMA before configuring receive DMA, while the 16-byte F1 pad
alone fills the four-word FIFO. The mismatched bit occupies the same physical
wire position under both byte orders. Prearming receive DMA removes that gap
without adding a DMA channel and advances the installer, but has not yet passed
all image reads. The SDK-like CS-high settling delay does not eliminate the
stale-looking result. PIO FIFO/execution/clock-divider resets and compiler
completion fences also leave stale-looking readbacks in their separate trials.
Restoring CHIPCOMMON after bulk accesses is the final change in the passing
combination. The smaller variant then establishes that endian, explicit timing,
FIFO-reset and additional fence changes are unnecessary for this diagnostic.
The production-runner test then establishes independent reception through the
native branch. It does not isolate a single root cause for every failed trial.
No retry acceptance or relaxed image verification is used.
