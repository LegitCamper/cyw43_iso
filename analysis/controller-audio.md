# LC3 payload rotation, 10-ms timing and CSA #2

Two bounded live runs transmit actual LC3 SDUs from CYW43439 to an independent
nRF54L15 raw receiver. The second run hops through all-enabled-map CSA #2
channels. Received SDUs decode to PCM on the host. These are controller radio
experiments with preloaded audio and a preconfigured receiver; standard discovery,
BIG synchronization and host-fed ISO data remain separate work.

## Captured results

| Property | Fixed channel run | Hopping run |
| --- | --- | --- |
| Capture directory under `work/` | `controller-audio-rf-probe` | `controller-hop-rf-probe` |
| Logged CRC-valid full packets | 27 | 27 |
| Four fixture frame counts | 7, 7, 7, 6 | 7, 7, 7, 6 |
| Checked successive timestamps | 26 | 26 |
| ADDRESS-to-ADDRESS range | 10000..10001 us | 10000..10001 us |
| ADDRESS-to-PHYEND | 248 us | 248 us |
| Channel selection | data channel 0 | CSA #2, event counters 0..26 |
| Decoded samples | 2160 mono samples | 2160 mono samples |

Each packet has header `00 1a` followed by one 26-byte LC3 SDU. Configuration is
BAP 8_2: 8 kHz, 10 ms, mono. Four distinct frames repeat, so this proves changing
payloads, not continuous live-source audio. Software CRC24 with BIS CRCInit
`0x123401` agrees with every captured Nordic CRC. AA is `0x718ebed6`.

The receiver captures RADIO ADDRESS and PHYEND events through DPPI into a
1-MHz 32-bit timer. Checks account for wrap, validate packet duration, and compare
payload order against elapsed event intervals. This establishes measured cadence
for these runs, without inferring RF timestamps from CPU callback counts.
RTT remains nonblocking and RX pauses during logging; this setup is not a
general lossless sniffer or long-duration synchronization test.

The hopping capture checks every channel and next-channel against a separate
Python CSA #2 reference. Initial channels are 1, 28, 5, 16, 29, 30. The receiver
computes them with the Rust `IsoChannels` implementation, while CYW43439 executes
the compiled Cortex-M3 `hardware/bis_channel.c` routine. Receiver starts with a
known event counter and stops on loss; acquired anchors and loss recovery are
not exercised here.

## Resident implementation

`hardware/controller_audio.S` wraps stock DTM phase zero and TX completion.
Phase zero sets reload 16 and supplies the first LC3 frame. After stock TX
completion, the wrapper verifies no removal is pending and setup succeeded,
then refills selector zero using ROM `0x44ed2`. Four 26-byte frames use 28-byte
aligned strides plus tail padding for the ROM's minimum 40-byte copy contract.
The HOP variant also computes the next channel, updates RF frequency, and sets
the matching whitening channel. Refill and retune complete before the next
observed packet in these captures.

Fixed-channel code occupies a guarded 512-byte allocation; hopping uses 1024.
The allocator changes its immediate reservation size while retaining the same
one-shot latch, in-controller interrupt mask, and expected-bound checks.
The checker assembles the 1024-byte allocator variant and compares it against
the exact two-byte runtime patch. Storage is retained until controller reset.

## Reproduction checks

```sh
python3 analysis/check-controller-tx.py --audio \
  --capture work/controller-audio-rf-probe/tx-instrumented.txt \
  --peer-capture work/controller-audio-rf-probe/receiver.txt
python3 analysis/check-controller-tx.py --hop \
  --capture work/controller-hop-rf-probe/tx-instrumented.txt \
  --peer-capture work/controller-hop-rf-probe/receiver.txt
python3 hardware/nrf54-dtm-rx/check-timestamps.py \
  work/controller-hop-rf-probe/receiver.txt
cargo run --features lc3 --example radio_audio -- decode \
  work/controller-hop-rf-probe/received.lc3 \
  work/controller-hop-rf-probe/received.s16le
```

Both directories retain actual received `received.lc3`, decoded `received.s16le`
and `received.wav` (mono 8000 Hz). The 270-ms waveform has peak magnitude 8368;
decoder success is not a Bluetooth qualification or listening-quality claim.
Source/ELF snapshots and logs preserve the tested versions.

Both runs terminate DTM, clear its event, restore callback tables, pass stock
HCI checks and reflash stock Pico firmware. Each saved Nordic full RRAM and
UICR readback compares byte-for-byte equal with its original backup.
