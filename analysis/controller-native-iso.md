# Native HCI ISO queue and RF completion credits

The experimental controller now accepts HCI transport type 5 on diagnostic
handle `0x0042`, copies complete 26-byte LC3 SDUs into an owned FIFO, and returns
Number Of Completed Packets events after radio completion. It retains the
single-BIS advertising/scheduling experiment described in
`controller-discovery.md`; handle establishment is not negotiated through
standard Create BIG commands.

## Live controller evidence

### Embassy production runner

`work/embassy-normal-runner-discovery-sdc-probe/` exercises the sibling
`rp2040_audio` application against native Embassy branch `cyw43-iso`.
Its RP2040 ELF SHA-256 is
`6e9352a789f1e117651bd7d0f15a1a521d0c725c826d529f9b77f1964dd0dd98`.
The guarded installer loads this repository's continuous-discovery blob, then
the real `Runner::run` and public `BtDriver` transmit 1,000 SDUs and receive
all 1,000 completion credits. The startup diagnostic pump is not used during
streaming. Host pauses for 80 ms after eight sends and 250 ms after 500 sends.

Stock Nordic SDC starts scanning before Pico startup. It discovers the broadcast,
establishes periodic/BIG sync and receives 950 exact 26-byte SDUs: 450 valid
records at sequences 55..504 and 500 at 527..1026. All 20 fixture frames appear
47 or 48 times, with cyclic order preserved. There are no missing log records
or sequence gaps in the full capture.

The receiver explicitly reports 22 status-2 slots at sequences 505..526 between
adjacent fixture frames 19 and 0. This supports padding recovery across the
250-ms refill pause. The 80-ms pause precedes receiver acquisition; its RF
recovery is not established by this capture. Payload identity cannot identify
an absolute host cycle because the fixture repeats. The receiver also reports
status 2 before the first valid packet and after the last valid packet; those
trailing records do not measure exact transmitter stop time or prove in-stream
packet loss. The capture proves receipt of 950 SDUs, not all 1,000.

Normal `Runner::run` owns the bus after startup, so the app does not read queue
counters or restore hooks itself. The outer runner resets/reloads Pico stock
firmware and passes two stock capability checks. Nordic full RRAM/UICR readbacks
match the original backups byte-for-byte. Raw logs, source/ELF/blob hashes and
`sdc-validation.json` are retained beside the captures. Transport fixes and
preceding diagnostic trials are in [embassy-transport.md](embassy-transport.md).
No LC3 decoder is part of this application or this interoperability check.

### Pico SDK sustained and late-join runs

The sustained run, `work/controller-continuous-iso-latejoin-sdc-probe/`, sends
1,000 host SDUs in an 11-second test. Sent, accepted, consumed, RF-completed,
reported and host-received credits all equal 1,000. It ends with 93 underrun
slots, an empty queue, no in-flight packet, no blocked input and no hardware
errors. Completion means transmitter completion, not receiver acknowledgment.

Receiver flashing starts only after the transmitter returns 20 RF credits.
The stock Nordic SDC receiver joins roughly eight seconds into streaming,
discovers the Broadcast Audio Announcement through normal scanning, and
establishes periodic sync at 20 ms and BIG sync at 10 ms. It receives 199
consecutive matching SDUs at ISO sequences 805 through 1003. The fixture repeats
20 distinct frames, so payload matching establishes cyclic order but does not
identify an absolute host cycle. This is a late-join test; it does not claim
reception of the earlier 801 host SDUs. The receiver continues logging lost SDUs
after the bounded transmission stops; its logs do not measure the RF stop time.

The exported `work/iso-blob-continuous/native.bin` matches the live source image
byte-for-byte (SHA-256
`be1197f5d05de61a28ba9bebb681882076c749f977931cbbebd35e87cdd92923`).
The 1,000-SDU limit belongs to the test host, not the resident blob.
Both devices were restored after this run: Pico stock firmware verification
and two stock capability checks pass; Nordic full RRAM/UICR readbacks match
the original backups byte-for-byte.

The earlier run, `work/controller-native-iso-discovery-sdc-probe/`, starts the
stock Nordic SDC receiver in normal scanning mode. It discovers Broadcast ID
`0x123456`, establishes periodic/BIG sync, and receives every fixture frame from
index 2 through 19 exactly and in order. Only startup frames 0 and 1 are
unobserved. No known-identity synchronization request is pre-armed. The checker
requires the received announcement before synchronization and explicitly rejects
preconfigured mode. Eighteen received frames decode to 1440 PCM samples in
`checked-audio/`; loss intervals are omitted from that waveform.

This run records 20 host sends/acceptances/consumptions/completions/reports/credits,
23 underrun slots, an empty final queue and no hardware errors. ISO sequences
7 and 12 carry consecutive fixture frames 7 and 8 across the deliberate refill
pause. This establishes scan-derived reception of native host-fed audio, not
only raw RADIO reception or emulator behavior.

`work/controller-native-iso-wire-probe/tx-instrumented.txt` records a 500-ms
controller run with 20 distinct host SDUs. All six totals agree at 20: sent,
accepted, consumed, RF-completed, reported and host-received credits. The final
queue is empty, no packet is in flight, no input block remains and no HCI
Hardware Error was seen. Twenty individual completion events sum to 20.

Host refill pauses for 80 ms after its eighth send, then resumes. The controller
records 29 underrun slots over the complete run, including the final drain.
Underrun sends zero-length LLID-1 padding, never a stale SDU. No HCI command
polling occurs during the streaming loop: IRQ completion wakes the transport
task, which sends the credit events needed to pass the initial four credits.
Stock version commands succeed before and after streaming.

`work/controller-native-iso-sdc-wire-probe/` adds an independent stock Nordic
SDC receiver. It establishes periodic/BIG synchronization and receives 15 of
the 20 distinct host frames, matching indices
`2,4,5,6,7,8,9,10,11,12,13,15,16,18,19` exactly and in order. Five frames are
unobserved; do not claim lossless delivery. The intentional pause is visible
between received ISO sequences 7 and 12, carrying consecutive fixture frames
7 and 8. Thus native host ingress, credit recycling, padding and recovery all
lead to independently received standard HCI ISO data. This receiver still
pre-arms the known advertiser identity.

Actual received frames decode to 1200 PCM samples in that directory's
`checked-audio/`. The WAV concatenates valid frames and omits loss intervals.
The host again accounts for all 20 completions and credits. Those counts mean
transmitter completion, not acknowledgment by a broadcast receiver.

The probe restores transport and lookup hooks, ends DTM, verifies event removal,
restores the radio callback table and checks stock HCI. Executable allocations
remain reserved until reset. Stock Pico firmware restoration and two capability
checks are retained beside the capture.
The Nordic full RRAM/UICR readbacks from both successful receiver runs also
match the original backups byte-for-byte.

## Implementation and ownership

The resident allocation is 4096 bytes. Radio wrappers and callback table retain
their earlier locations; CSA #2 is at +640, discovery PDUs at +1024/+1088/+1152.
The preparation stub at +1280 calls the consumer at +3328. Native transport
occupies +2048 (852 bytes), and the 148-byte credit helper starts at +3072.
The native variant's 504-byte consumer repeats three discovery rounds at 30-ms spacing
before periodic/BIS start at 75/78.75 ms, for normal scan-derived acquisition.
The earlier successful native capture retains its exact source/ELF snapshot
with the 464-byte, single-round helper; it does not test this later prelude.
The scan-derived capture exercises the current repeated prelude.

The continuous variant uses a 600-byte consumer at the same offset. After the
same startup prelude, each 20-ms cycle schedules periodic advertising at 0 us,
BIS at 3750 us, primary advertising at 7500 us, auxiliary advertising at
11250 us and BIS at 13750 us. Primary channels rotate through 37, 38 and 39.
Each auxiliary SyncInfo points to the next periodic event; BIGInfo advances
its payload count by two per cycle. BIS spacing remains 10 ms while discovery
remains available to receivers that start later.

Queue metadata starts at +1792: head, tail, accepted, rejected, consumed,
underrun, RF-completed, in-flight. Five 40-byte slots at +1824 provide four usable
entries. Each padded slot satisfies the ROM copy routine's aligned minimum
40-byte read. The task publishes head only after the SDU copy; the radio consumer
publishes tail only after the ROM copy. DMB barriers separate those operations.
Only the transport task produces SDUs; the vendor-command producer is absent.

RF completion increments the ISR-owned completed counter. The task owns the
last-reported snapshot at +2092. It emits a count of 1..4 and advances the
snapshot only after event ownership transfers successfully to ROM. Allocation
failure leaves credits pending. The ISR uses the stock allocation-free event
bit primitive to wake the task, never allocates an event itself.

LE Read Buffer Size V2 preserves stock LE ACL fields and reports ISO data length
30 and four packets. Input supports PB Complete, no timestamp, status Valid,
one fixed handle and 26-byte SDUs. Unsupported or malformed ISO remains blocked
for reset recovery; generalized rejection, fragmentation and stream negotiation
remain outside this experimental path.

The first live attempt exposed a host command-buffer bug: the SDK transport
writes a four-byte SDIO prefix before the caller's HCI packet and reads rounded
words. `tx_command` now copies into an aligned, zero-padded buffer with four
bytes of headroom and validates the command length. The original failure
corrupted adjacent saved ACL fields, despite a correct controller V2 response.
Failed attempts remain under `work/controller-native-iso-{sdc,buffer}-probe/`.

## Checks

```sh
python3 analysis/check-controller-tx.py --continuous --output-blob work/iso-blob-continuous \
  --capture work/controller-continuous-iso-latejoin-sdc-probe/tx-instrumented.txt \
  --peer-capture work/controller-continuous-iso-latejoin-sdc-probe/receiver.txt \
  --require-discovery
uv run --with unicorn python analysis/check-discovery-packet.py --continuous
cargo run --quiet --features lc3 --example radio_audio -- generate-native work/native-audio.lc3
python3 analysis/check-controller-tx.py --native \
  --capture work/controller-native-iso-discovery-sdc-probe/tx-instrumented.txt \
  --peer-capture work/controller-native-iso-discovery-sdc-probe/receiver.txt \
  --require-discovery
python3 analysis/check-sdc-capture.py \
  work/controller-native-iso-sdc-wire-probe/receiver.txt \
  --fixture work/native-audio.lc3 --native-fixture
uv run --with unicorn python analysis/check-discovery-packet.py --native
uv run --with unicorn python analysis/check-transport-ingress.py --emulate
uv run --with unicorn python analysis/check-iso-credits.py --emulate
```

The 520-byte fixture contains 20 distinct valid LC3 frames. Generation checks
uniqueness and local decoding; its first four frames remain identical to the
preloaded radio fixture. The source checker rebuilds all embedded code, verifies
two relocations and the allocator patch, and checks live completion totals.
ARM checks exercise FIFO wrap, copy-before-release, delayed completion credits,
counter wrap, underrun, invalid indices, failed event allocation and retry.
ROM payload copying and task wake are mocked in consumer emulation; live checks
remain necessary for their scheduling and transport behavior.
The source checker builds the current image; historical capture snapshots
preserve the versions actually transmitted in earlier experiments.
