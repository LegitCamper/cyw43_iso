# BIS radio identity and whitening

CYW43439 transmitted custom payloads using a BIS-derived Access Address,
CRCInit and enabled data-channel whitening. A separately configured nRF54L15
received full CRC-valid packets. This establishes radio identity and whitening
for one channel; the run still uses stock DTM recurrence and does not establish
a synchronized BIS or LE Audio broadcast.

## Observed configuration

| Field | Value |
| --- | --- |
| Seed Access Address | `0x8c88bed6` (fixed test fixture) |
| BaseCRCInit / BIS number | `0x1234` / 1 |
| Derived Access Address | `0x718ebed6` |
| Derived CRCInit | `0x123401` |
| CSA #2 channel identifier | `0xcf58` |
| PHY / channel | LE 1M / data channel 0, 2404 MHz |
| Whitening | enabled, logical channel 0 |
| Header / payload | `00 1f` / `00 01 ... 1e` |
| Received CRC register | `0x6a2c0d` |

The header corresponds to complete unframed ISOAL data (LLID 0) with CSSN,
CSTF and RFU zero. No BIG Control procedure is exercised. Seed generation and
device-wide uniqueness requirements remain outside this fixed fixture.

The controller's phase-zero wrapper calls stock setup, then applies:

- AA at `0x318b70` and CRCInit at `0x318ba4`.
- `0x300000` ORed into `0x318b6c`, observed final value `0x01306340`.
- Logical whitening channel 0 at `0x318bf4`.
- HCI test RF channel 1, which means 2404 MHz in test-mode 2-MHz units.

Nordic independently uses AA/CRC above and nRF54's `DATAWHITE` polynomial
`0x89`, initializer `0x40 | channel`. Unlike older Nordic parts, this is not
the old `DATAWHITEIV` register. Reception corroborates the combined controller
configuration; it does not isolate the meaning of each control-mask bit.

## Evidence and checks

`work/controller-bis-rf-probe/` retains source/ELF snapshots and logs.
`receiver.txt` contains 165 complete CRC-valid payload records. Receiver counters
report 209 received / 209 valid; RTT drops logs and RX pauses while logging.
Software CRC24 with initializer `0x123401` reproduces every logged checksum.
Controller telemetry records 403 callbacks and stock completions; no RF cadence
measurement is inferred from these counts.

```sh
python3 analysis/check-controller-tx.py --bis \
  --capture work/controller-bis-rf-probe/tx-instrumented.txt \
  --peer-capture work/controller-bis-rf-probe/receiver.txt
```

LE Test End clears the event; callback table restoration and stock HCI version
check pass. Pico stock firmware is restored. Saved Nordic full RRAM/UICR
readbacks compare byte-for-byte equal with the original backups under
`work/nrf54-dtm-peer-backup/`.

## Remaining scheduling work

The next experiment changes the DTM reload to 16 nominal 625-us ticks and
rotates four LC3 SDUs after confirmed TX completion. Hardware event timestamps
must establish cadence and refill behavior. Then add CSA #2 hopping and receiver
anchor/event tracking. Standard discovery needs extended/periodic advertising,
SyncInfo, BASE and BIGInfo; host ISO ingress and credits must connect to the
resident transmit queue before claiming an integrated implementation.

`hardware/bis_channel.c` supplies the first-subevent CSA #2 calculation for a
full 37-channel map and NSE=1. Its 120-byte Cortex-M3 build has no undefined
symbols and identical bytes at two SRAM link addresses. The offline checker
compares 262144 native executions with a separate bit-reversal reference and
Core sample vectors; this does not itself prove controller execution.

```sh
python3 analysis/check-bis-channel.py --output work/bis-channel
```
