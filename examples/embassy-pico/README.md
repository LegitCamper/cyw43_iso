# Pico W native ISO probe

Real RP2040 Embassy executable: boots signed `.0065` Bluetooth firmware with
the validated 112-byte zero append at `0xd0f90`, installs the native controller
image, sends twenty fixture SDUs via
public `BtDriver`, validates RF-completion credits, pauses 80 ms after SDU 8,
checks controller counters, stops radio and restores stock hooks. Uses no heap.
Only the Bluetooth runner pump runs; Wi-Fi control remains idle.

From repository root, using an existing Embassy firmware checkout:

```sh
python3 examples/embassy/prepare-runtime.py \
  --firmware-dir /var/home/sawyer/projects/embassy/cyw43-firmware
DEFMT_LOG=info DEFMT_RTT_BUFFER_SIZE=32768 cargo build --release \
  --manifest-path examples/embassy-pico/Cargo.toml \
  --target thumbv6m-none-eabi --target-dir work/embassy-pico \
  --config "patch.crates-io.cyw43.path='$PWD/work/embassy-runtime/cyw43-0.7.0'"
```

ELF: `work/embassy-pico/thumbv6m-none-eabi/release/cyw43-iso-embassy-pico`.
Build links `link-rp.x`: `.boot2` must be at `0x10000000` and vectors at
`0x10000100`. Pins are Pico W power 23, CS 25, data 24, clock 29;
PIO0 SM0 and DMA_CH0 implement SPI. Firmware inputs are hash checked by the
preparation script. Firmware licensing remains with the source distribution.

RTT emits `EMBASSY_ISO` records. For the initial build, control block is
`0x20000008`, buffer `0x20005d64`, length 32768; resolve ELF symbols after
rebuilding. No USB serial is configured. Healthy final record is
`EMBASSY_ISO restored=true pass=true`, with sent/accepted/consumed/RF/reported/
credits all 20, queue empty, inflight/blocked zero and positive underruns.
An external standard receiver is still required to prove over-air decoding.

Any error, panic, cancelled operation or missing final record requires an
external chip restart and firmware reload, not just HCI Reset. The live runner
must restore known stock firmware in
its cleanup path even if RTT times out. Internal hook restoration retains
allocated controller SRAM until reset. This is experimental fixed native
BIS transport, not general HCI ISO negotiation or Wi-Fi coexistence support.

Standalone driver boundary checks and source provenance are in
`../embassy/README.md`.
