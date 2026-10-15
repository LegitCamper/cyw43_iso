# Codec package composition

Run `cargo run --manifest-path examples/lc3-composition/Cargo.toml --locked`.

This combines upstream `lc3-codec` 0.2.0 with its default `alloc` feature (as used
by `trouble_audio`) and this crate's allocation-free `cyw43-lc3-codec` fork in one
binary. Both directions encode/decode eight 16 kHz mono frames and assert equal
output. Distinct package identities prevent Cargo from changing the fixed-channel
backend's API through feature unification.

This checks codec composition only; it does not instantiate GATT or radio links.
