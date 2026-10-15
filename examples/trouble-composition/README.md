# Trouble LE Audio composition

Run from this directory so Cargo loads its nightly toolchain and solver setting:

```sh
cd examples/trouble-composition
cargo run --locked
cargo clippy --locked --all-targets -- -D warnings
```

The executable checks an actual `trouble_audio::cis::RawLc3` receive path with
this repository's fixed-channel decoder: eight 16 kHz/10 ms SDUs, sequence-number
wrap, one missing SDU concealed before the next valid frame, and resumed decoding.
PCM matches a reference decoder explicitly given the lost frame. Replayed frames,
large gaps, wrong ASE IDs, and incorrect payload lengths fail before codec state
changes. Input is generated PCM, with no radio or audio device involved.

The generated single-BIS BASE also passes Trouble's actual broadcast parser.
The check verifies presentation delay, codec configuration, Streaming Audio
Contexts, and BIS index, then compares both advertising service-data wrappers
against Trouble's encoders. Advertising and BIG creation are not exercised.

Our encoded LC3 configuration also round-trips through Trouble's actual ASCS GATT
value type. `configure_lc3` in `src/lib.rs` compiles ASCS discovery, notification
subscription, characteristic writing, and response matching against the real GATT
APIs. It uses this repository's response parser to handle the ASCS global-error
count `0xff`, which the pinned profile parser does not handle. GATT procedures
are compile-checked; this executable does not connect to a peer. Caller must run
the GATT client task, negotiate PACS/security, apply timeout/disconnect handling,
and wait for Codec Configured ASE status before proceeding to QoS.

For a real receiver, use `CisManager::new_passthrough()` and feed frames from
`receive_lc3()` to `decode_received`, with one decoder/sequence state per mono
ASE. At most four missing SDUs are concealed; larger gaps require stream reset.
The first valid SDU establishes the sequence baseline. Initial loss and playout
deadlines require a presentation timer. Sample output callbacks receive both
concealed and valid PCM; the example does not silently discard lost-frame PCM.

Dependencies are pinned to the existing stack: `trouble_audio` commit
`a6a374c6c42997d46c6a7ad2badd9bef103afe11`, `trouble-host`
`1f154caf2c4e1fb46bfcec0677c8545457c9b85b`, and `bt-hci`/`bt-hci-transport`
`a3b9b9785ef6e8269db31e1431f7d0710a6265cc`. The two HCI patches preserve that
stack's variable-length CIG command return and CIS Test NSE field. No sibling
checkout is needed. Upstream `lc3-codec` retains `alloc`; our renamed
`cyw43-lc3-codec` package remains allocation-free without feature unification.

The example requires nightly because the existing profile stack uses
`generic_const_exprs`. Tested with `rustc 1.101.0-nightly (db8f076d2 2026-10-03)`
and `-Znext-solver=no`. The lockfile pins dependencies; nightly itself is floating.

Transport versions remain distinct: this repository's `embassy` adapter uses
crates.io CYW43 0.7 with `bt-hci` 0.8. The pinned Trouble profile stack uses
`bt-hci` 0.11 and `bt-hci-transport` 0.1; its upstream Pico example bridges the
git Embassy CYW43 driver's `bt-hci` 0.9 API to that byte transport. This composition
does not connect those controller types or claim working CIS/BIS radio on CYW43.
