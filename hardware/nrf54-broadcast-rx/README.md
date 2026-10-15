# Stock SDC broadcast receiver

Independent nRF54L15 Connect Kit receiver using Nordic's SDC controller binary
and the pinned Rust HCI wrapper. No custom RADIO programming, heap, GATT stack,
or on-device LC3 codec. Commands and received events follow standard Bluetooth
HCI: extended scan, periodic sync, BIG sync, and ISO output data path.

Build from this directory; no command here accesses a probe:

```sh
cargo build --release --locked
cargo clippy --release --locked -- -D warnings
rustc --edition 2024 --test src/discovery.rs -o /tmp/nrf54-discovery-check
/tmp/nrf54-discovery-check
```

Default mode discovers Broadcast ID `0x123456`, SID 3, random advertiser address
`C6:05:04:03:02:01`. It then requests periodic synchronization. This needs a
subsequent discovery advertisement carrying fresh SyncInfo; a single initial
primary/auxiliary pair may not suffice.

For a transmitter with only one discovery pair, explicitly pre-arm the known
advertiser before starting it:

```sh
cargo build --release --locked --features preconfigured
```

For longer captures, prefix the build with `DEFMT_RTT_BUFFER_SIZE=32768`.
The resulting ELF has a 32 KiB RTT buffer; logging remains nonblocking, so this
reduces dropped evidence without proving lossless capture.

This mode logs `preconfigured-identity-sync-armed`. It tests periodic/BIG
synchronization using a configured identity; it does not prove host discovery.
Periodic and BIG sync timeouts are `0x4000` (163.84 s), allowing transmitter
flashing/startup after the receiver is armed. Startup logs name each HCI command
and opcode so a controller rejection identifies its failing stage.
Scanner sets static random address `C0:65:64:63:62:61` before enabling scanning;
the board need not have a provisioned public address. This fixed lab identity
must become device-unique before deploying multiple receivers.
ELF: `target/thumbv8m.main-none-eabihf/release/nrf54-broadcast-rx`.

Receiver accepts only the current unencrypted, unframed one-BIS fixture:
8 kHz/10 ms/26-byte mono LC3, 10 ms ISO interval, BN=NSE=IRC=1. It checks the
complete BASE and BIGInfo report before requesting BIS 1. ISO logs include
sequence, packet status, SDU length, validity, and LC3 bytes. `valid=true` means
an intact 26-byte HCI SDU, not an audio-content match. Compare logged bytes with
transmitted frames and check sequence gaps separately. RTT is nonblocking, so
missing log records alone do not prove radio loss. Fragmented BASE/ISO is outside
this fixture's scope. Sync loss or command failure resets and rearms after 1 s.

Pinned dependencies: nrf-sdc `d6f66312ecf34c086babb2a56e3319d8da902fc9`,
bt-hci `a3b9b9785ef6e8269db31e1431f7d0710a6265cc`, Embassy
`03c826f2544220cb44b2dec05f5d636a80d39382`. The SDC wrapper gates extended scan,
periodic sync, and BIS sink behind both central/peripheral features; the
controller is configured only for receiving. Board initialization follows the
existing Trouble nRF54L15 broadcast example. Builds use stable Rust; no SDK
installation or sibling checkout is required after Cargo fetches pinned sources.

Build and parser checks pass. Live testing confirms random-address scanning and
preconfigured sync arming; the public-address variant rejected scan enable with
Invalid HCI Command Parameters. Captures:
`work/controller-discovery-sdc-diagnostic-probe/receiver.txt` and
`work/controller-discovery-sdc-random-probe/receiver.txt` from the repository root.
Later `work/controller-discovery-scan-sdc-probe/receiver.txt` records successful
periodic and BIG synchronization, then 22 exact fixture SDUs. This run uses
preconfigured identity and proves no scan-derived host discovery. Full evidence
and limitations belong to the repository's discovery report.

Check the saved receiver log from the repository root:

```sh
python3 analysis/check-sdc-capture.py --self-test
python3 analysis/check-sdc-capture.py \
  work/controller-discovery-scan-sdc-probe/receiver.txt \
  --fixture work/radio-audio.lc3 --last-transmitted-seq 24
```

Sequence 24 is a caller-supplied cutoff, not an independently measured last
transmitted sequence. The checker separates 22 valid and two lost SDUs through
that cutoff from 878 later lost reports; receiver log timestamps do not measure
RF timing. Add `--output NEW_DIRECTORY` to extract valid SDUs and decode mono
8 kHz PCM/WAV. Output omits lost slots; `capture.json` preserves each sequence
and status, so the waveform is not a loss-concealed playback timeline.

For the native ingress experiment, use `--native-fixture --fixture
work/native-audio.lc3`. That mode requires 20 distinct fixture frames, more than
four observed SDUs, strictly increasing matched fixture indices, and reception
after a loss or sequence gap. It reports unobserved initial/intermediate/final
frames. Fixture index is independent of ISO sequence because underrun padding
advances the latter. Recovery in a receiver log alone does not establish why
the gap occurred; correlate it with the transmitter's intentional pause.
