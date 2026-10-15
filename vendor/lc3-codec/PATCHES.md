# Local LC3 fixes

Source: crates.io `lc3-codec` 0.2.0, Apache-2.0, upstream
<https://github.com/ninjasource/lc3-codec>.

Packaged commit: `65bfd7752d151a12d1e399e0edf4b84be136f083`.
Crate archive SHA-256:
`ba01b209b6df33864b21e0d987689ad8e497b9a49f59bf75be0c1cf3bb47f2f4`.
Both were checked against the downloaded crates.io package.

Only library source, Cargo manifest, README, and LICENSE are vendored. Cargo uses
this directory through a direct path dependency.

The local package is named `cyw43-lc3-codec` with `publish = false`; consumers
keep the `lc3-codec` dependency alias. This prevents Cargo feature unification
with upstream `lc3-codec` when a profile stack enables its `alloc` feature.
Upstream changes encoder/decoder type signatures under `alloc`; our fixed-channel
backend requires the no-allocation signatures. Source version remains 0.2.0.

`src/encoder/bandwidth_detector.rs`: use `sample_freq_ind.saturating_sub(1)` for
table selection. At 8 kHz the index is zero and subtraction previously panicked
before encoding. `run()` already handles 8 kHz before accessing those tables, so
selecting table zero has no effect on encoded output. Other sample rates retain
their original indices.

`src/encoder/spectral_noise_shaping.rs`: narrow-band padding duplicates the first
four of 60 input bands, then copies the remaining 56 bands, producing 64 outputs.
The original second loop incorrectly copied 60 remaining bands and overran both
buffers at 8 kHz/7.5 ms. Its upper bound is now `num_bands - diff`.

`src/encoder/residual_spectrum.rs`: make the returned `ResidualBits<'_>` lifetime
explicit for current Rust's `mismatched_lifetime_syntaxes` lint; no behavior change.

`src/decoder/buffer_reader.rs`: port the verified local `ble_audio_test` fix:
tail-bit reads use `buf.len().checked_sub(byte_index + 1)` so empty/exhausted
input returns an error. Arithmetic decoding may read ahead at the head, so head
and tail cursor overlap must remain allowed. Empty/exhausted regressions are
retained in the vendored module and exercised by the root codec tests.

Regression: `cargo test --features lc3 --test le_audio_codec` covers encode and
decode at all six sample rates, with both 7.5 ms and 10 ms frames.
