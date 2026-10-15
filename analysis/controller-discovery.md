# CYW43439 discovery and stock-controller BIS reception

CYW43439 transmits a cleartext single-BIS broadcast that an independent Nordic
SDC controller synchronizes to and delivers through standard HCI ISO. This is
a bounded preloaded-audio experiment, with a known advertiser identity on the
receiver. It does not yet establish ordinary host discovery or sustained
host-fed audio.

## Evidence

`work/controller-discovery-logged-rf-probe/` retains a raw Nordic RADIO capture:
53 complete CRC-valid packets: one primary, one auxiliary, 26 periodic and
25 BIS. The receiver follows AuxPtr, SyncInfo, BIGInfo and BASE, deriving radio
identities and channels from received fields. Independent software CRC24 agrees
with every logged packet. This receiver uses custom RADIO code.

`work/controller-discovery-scan-sdc-probe/` instead runs the stock Nordic SDC
controller binary through standard typed HCI commands. Its `receiver.txt`
records successful LE Periodic Advertising Sync Established and LE BIG Sync
Established events, matching BASE and BIGInfo, and intact 26-byte ISO SDUs.
The receiver uses no custom RADIO configuration. It pre-arms synchronization
for random advertiser `C6:05:04:03:02:01`, SID 3; this is weaker acquisition
evidence than discovery from an unknown advertiser.

The capture contains 22 valid SDUs at sequences 2 and 4 through 24. Sequence 1
and 3 report lost data; 878 later loss reports extend through sequence 902.
The transmitter intentionally stops after a bounded burst, but its snapshot
does not establish the exact last transmitted sequence. Splitting at 24 is a
caller-selected cutoff, not measured stop timing. Count only intact SDUs as
delivered audio. RTT timestamps are task log
times, not hardware RF timestamps. The earlier hopping experiment measures
10-ms cadence separately; see `controller-audio.md`.

## Packet schedule

Three primary ADV_EXT_IND packets use channels 37, 38, 39 at relative times
0, 3750 and 7500 us. Their AuxPtrs target one AUX_ADV_IND at 11250 us using
offsets 375, 250 and 125 in 30-us units. SyncInfo targets the first periodic
packet at 15000 us. BIGInfo targets BIS at 18750 us. Periodic and BIS packets
then repeat every 10000 us, separated by 3750 and 6250 us.

Periodic access address is `0x12345678`, CRCInit `0xabcdef`, initial event
counter `0x1234`. BIS 1 derives address `0x718ebed6`, CRCInit `0x123401`, and
channel ID `0xcf58`. Both use CSA #2 across the full data-channel map.
Broadcast ID is `0x123456`. BASE describes mono LC3 at 8000 Hz, 10 ms and
26 octets per frame. BN=NSE=IRC=1, PTO=0, unframed, unencrypted.

`hardware/controller_audio.S` retains stock DTM event ownership and replaces
packet preparation after completion. `hardware/discovery_packet.c` sets
address, CRC, frequency, whitening and the next reload interval. The reload
is consumed one packet ahead by the stock trigger path. Generated PDU fixtures
come from `examples/broadcast_fixture.rs`; four LC3 frames come from
`examples/radio_audio.rs`.

## Checks

```sh
python3 analysis/check-controller-tx.py --discovery \
  --capture work/controller-discovery-logged-rf-probe/tx-instrumented.txt \
  --peer-capture work/controller-discovery-logged-rf-probe/receiver.txt
python3 analysis/check-controller-tx.py --scan \
  --capture work/controller-discovery-scan-sdc-probe/tx-instrumented.txt
uv run --with unicorn python analysis/check-discovery-packet.py
cargo run --quiet --features lc3 --example radio_audio -- generate work/discovery-audio.lc3
python3 analysis/check-sdc-capture.py \
  work/controller-discovery-scan-sdc-probe/receiver.txt \
  --fixture work/discovery-audio.lc3
```

The checker compares resident images against freshly compiled sources and Rust
PDU builders, verifies allocation and restoration markers, and checks the raw
capture independently. ARM emulation covers both primary schedules at two
allocation addresses, including channel counter wrap; ROM copy is mocked.
The stock-SDC checker verifies both sync events, BASE/BIGInfo, every intact
SDU against the independently generated fixture, and sequence order. Its
`--self-test` checks rejection paths. Actual received SDUs decode to 1760 PCM
samples in `work/controller-discovery-scan-sdc-probe/checked-audio/`.
That waveform concatenates valid frames and omits lost intervals.

The SDC transmitter capture confirms DTM stop, event removal, callback restore,
stock HCI operation and retained storage until reset. Separate stock Pico flash
and capability logs remain beside the capture. The outer hardware batch also
restored Nordic full RRAM and UICR, both compared byte-for-byte against the
original backups. Those readbacks are in
`work/controller-discovery-scan-rf-probe/receiver-restored-{rram,uicr}.bin`.

Subsequent native HCI ISO ingress/credits, continuous discovery and sustained
streaming with late receiver acquisition are documented in
`controller-native-iso.md`. Remaining work includes normal controller event
ownership, BIG control procedures, CIS and live Embassy integration. Successful reception of this
constrained broadcast does not establish a general ISO-capable or qualified
controller.
