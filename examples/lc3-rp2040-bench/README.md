# RP2040 LC3 timing benchmark

Bounded Pico W benchmark of the vendored `cyw43-lc3-codec` backend: mono 8 kHz,
10 ms, 80 input samples, 26 encoded bytes. Twenty distinct square-wave frames
use the same input pattern as `radio_audio generate-native`. Each frame is
encoded then decoded on core 0; one additional decode exercises PLC. The CPU
uses Embassy RP2040's default 125 MHz clock, reported at runtime. RP2040 ROM
float intrinsics are enabled. No heap, CYW43, radio, host wrapper, or executor
runs in this image.

`embassy_time::Instant` reads the RP2040 hardware timer at 1 MHz. Timed regions
cover codec calls and timer-read overhead. Input generation, validation and
logging occur outside those regions. First-frame startup cost remains included;
no warmup is discarded. Per-frame and aggregate results report encode, decode,
and their sequential sum against separate 10,000 us budgets. Passing individual
budgets does not imply their sum fits. A timer-read overhead sample is reported
but not subtracted. Assertions check nonzero decoded audio and distinct SDUs.

Build only; these commands do not access hardware:

```sh
cargo build --release --locked
cargo clippy --release --locked -- -D warnings
python3 check-capture.py --self-test
```

ELF: `target/thumbv6m-none-eabi/release/lc3-rp2040-bench`.
Memory layout is shared with `../embassy-pico/memory.x`; `link-rp.x` places
`.boot2` at `0x10000000`, with the vector table at `0x10000100`.

Hardware owner can run this ELF with probe-rs on the Pico W's RP2040 and capture
RTT until `LC3_BENCH done`. Allow up to 30 seconds; the firmware sleeps after its
bounded run. Preserve and restore the board's previous firmware. Expected logs:
one `LC3_BENCH start`, twenty `LC3_BENCH frame`, one `LC3_BENCH result`, and
one `LC3_BENCH done`. Buffer is 32 KiB, nonblocking; a missing line makes the
capture incomplete.

```sh
python3 check-capture.py /path/to/receiver.txt
```

Checker validates every frame index, aggregate timing arithmetic, budget-miss
counts, configured clock and nonzero audio. Missed deadlines remain valid
benchmark results. Build success alone supplies no throughput evidence; even a
successful hardware run covers this fixture without concurrent radio load,
not worst-case real-time LE Audio. The fixed backend is measured directly:
root crate's allocator-using modules and checked SDU wrapper are excluded.
