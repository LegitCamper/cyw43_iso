# Independent LC3 reference

Generated with Google liblc3 commit
`efe84d3e0ece9e54591098148da3d10f5503b84e` (Apache-2.0), built with `make liblc3`.
No third-party audio content: input is a generated 1 kHz sine wave.

- PCM sample format: signed 16-bit, 16 kHz, mono.
- One repeated 160-sample frame: `int(10000 * sin(i * tau / 16))`, `i = 0..159`.
- LC3 configuration: 10 ms, 40 octets/frame, eight consecutive frames.
- `google-16k-10ms-40b.lc3`: all eight raw frames, no container header.
- `google-16k-final-frame.pcm`: Google's decoder output for the eighth frame,
  signed little-endian 16-bit PCM.

Generation uses `lc3_setup_encoder(10000, 16000, 0, memory)` and eight calls to
`lc3_encode(encoder, LC3_PCM_FORMAT_S16, pcm, 1, 40, frame)`. Decode all eight
frames with `lc3_setup_decoder(10000, 16000, 0, memory)` and
`lc3_decode(decoder, frame, 40, LC3_PCM_FORMAT_S16, pcm, 1)`; retain the final PCM.

Root regression compares this independently encoded stream decoded by the Rust
backend with Google's reference PCM. Measured agreement: 59.41 dB SNR. A manual
reverse-direction check also fed the Rust encoder's eight frames to Google and
measured 21.19 dB SNR versus the original tone after delay alignment. These are
interoperability smoke tests, not Bluetooth codec qualification.
