# Controller-resident payload transmitted over air

CYW43439 transmitted a complete custom 31-byte SRAM payload through its stock
Direct Test Mode event. A separate nRF54L15 received the full packet with valid
CRC. This proves custom RF payload transmission; it does not establish BIS,
CIS, ISO timing, or LE Audio interoperability.

## Evidence

Retained run: `work/controller-tx-rf-logged-probe/`. This directory includes
source and ELF snapshots, `source-and-elf.sha256`, transmitter captures,
`receiver.txt`, and restoration readbacks. Later source changes do not describe
these saved binaries.

- Radio: BLE 1M, 2402 MHz, AA `0x71764129`, CRCInit `0x555555`, whitening off.
- Stock control: header `00 1f`, 31-byte PRBS9 payload beginning `ff c1 fb e8`;
  Nordic RXCRC `0xe90e88`.
- Custom: header `00 1f`, every payload byte `00 01 ... 1e`;
  Nordic RXCRC `0xabb0ee`.
- Capture contains 129 stock and 104 custom CRC-valid packet records. Software
  CRC24 over each full header and payload agrees with its Nordic RXCRC.
- Receiver counters report 830 received / 830 CRC-valid across both runs.
  Nonblocking RTT drops logs and reception pauses during logging; these counters
  do not mean 830 matched payload records or establish a packet-loss rate.
- Instrumented controller records 403 TX callbacks in interrupt context and
  403 stock completions. Host requests a 250-ms window; no RF timestamps were
  measured, so this count cannot establish exact cadence or jitter.

The phase-zero wrapper temporarily replaces the stock payload pointer with a
guarded controller SRAM buffer, calls stock setup, and restores the pointer.
Stock setup copies the payload into its radio window. Repeated DTM packets
reuse that copy. TX completion instrumentation calls the stock callback.

Both host backplane reads and controller-side reads of `0x370800` repeated the
first word. Full RF capture shows the transmitted bytes are correct despite
that readback behavior. Payload aperture readback must not be used to certify
all transmitted bytes.

## Checks and restoration

```sh
python3 analysis/check-controller-tx.py \
  --capture work/controller-tx-rf-logged-probe/tx-instrumented.txt \
  --peer-capture work/controller-tx-rf-logged-probe/receiver.txt
python3 analysis/check-controller-tx.py \
  --capture work/controller-tx-rf-logged-probe/tx-control.txt
```

The checker also verifies the clean ROM hash, original callback table,
relocation-independent assembly, source bytes, guards and restoration markers.
Its source checks apply to the current implementation; the saved source/ELF
snapshots identify the implementation actually used in this run.

Both transmitter runs complete LE Test End, clear the event, restore the table
pointer, answer the stock version command, and reflash stock Pico firmware.
Allocated code remains resident until reset because scheduler objects can retain
callback pointers. Prematurely freeing it would risk executing released memory.

Connect Kit B's full `0x17d000`-byte RRAM and 4096-byte UICR were backed up.
Saved `receiver-restored-rram.bin` and `receiver-restored-uicr.bin` compare equal
to `work/nrf54-dtm-peer-backup/connectkit-b-{rram,uicr}.bin` byte for byte.

## Next transport boundary

`dtm-timing.md` identifies the repeat reload and candidate payload-update point.
`radio-whitening.md` identifies candidate whitening controls. Both require RF
validation. BIS additionally needs its derived address/CRC, header, CSA #2,
event timing and synchronized receiver, followed by periodic advertising and
BIGInfo for standard discovery.

`examples/radio_audio.rs` generates four distinct 26-byte LC3 SDUs (BAP 8_2,
mono 8 kHz / 10 ms) in 104 bytes, and decodes ordered SDUs to signed 16-bit
little-endian PCM. It provides a local assert-based check for forthcoming audio
payload experiments. It does not transmit or repair missing radio timing.

```sh
cargo run --features lc3 --example radio_audio -- generate work/radio-audio.lc3
cargo run --features lc3 --example radio_audio -- decode \
  work/radio-audio.lc3 work/radio-audio.s16le
```
