# Phase 1: stock CYW43439 Bluetooth Patchram

## Artifact identity

- Input: `43439A0_btfw.bin`
- SHA-256: `ce1992c1a6a16ae51bc012439486e9fb212623eca92d9e82a8090c2acf7ef1df`
- Version: `CYW4343A2_001.003.016.0031.0000_Generic_SDIO_37MHz_wlbga_BU_dl_signed`
- Container records: 33
- Exact Rust parse/re-encode: verified byte-for-byte

## Sparse layout

| Controller address | Length | Observation |
| --- | ---: | --- |
| `0x000d0200..0x000d0cd4` | 2772 | Thumb-2 Patchram code |
| `0x002038d4..0x002038d8` | 4 | Value `0x0021e000`, points at config region |
| `0x00204164..0x00204168` | 4 | Value `0x0005f7ed`, likely Thumb code pointer; role not yet verified |
| `0x0021e000..0x0021e042` | 66 | `BRCMcfgS` header/config |
| `0x0021f000..0x0021fc22` | 3106 | `BRCMcfgD` configuration/patch data |

## REA observations

- ARM ELF wrapper at observed Patchram address yields 32 procedures.
- Patch code makes extensive calls into controller ROM below `0x000d0000` and accesses controller RAM near `0x00200000`.
- Example recovered functions reference ROM entry points such as `0x000444d6`, `0x0004d9d6`, `0x00076554`, and `0x00085f58`.
- Existing blob is an overlay, not a standalone Bluetooth controller image. A source rewrite must preserve or replace ROM ABI calls, global data layouts, interrupt behavior, and radio scheduling.

REA evidence:

- Overview: `ev_6967d606bdc1a70ec02d607878a4b65b7cec3d535e3dd9a8c0c54a54584fdc96`
- Procedure inventory: `ev_4a120b79eb17ad1348c8501f75784ca7443f6d0072b7aa3ddde8e6ea77eda561`
- Batch decompilation: `ev_dd6e0daae49638e6ac056e379d50b6a9ef1074a1827a5062a98cd725ec7a6844`
- Portable bundle: `analysis/patchram-evidence.json`

## ROM capture (corrected)

Flash `work/hardware-sdk22/picow_btrom_dump.uf2`, capture USB serial output,
then decode it:

```sh
cargo run -- decode-dump capture.txt work/cyw43439-btrom-clean.bin
```

The dumper reads controller offsets `0x00000000..0x00090000` through backplane
address `0x19000000 + offset`. Each 64-byte record and
full image carry CRC32. The SPI driver's maximum transfer is 64 bytes; the
original 256-byte probe corrupted its own read buffer.
The resulting ROM image provides external call bodies and signatures needed for
safe function-by-function Rust reconstruction.

## Reconstruction boundary

First Rust target should reproduce the existing 2772-byte Patchram overlay and
its config records. ISO support comes after behavioral equivalence, because the
stock overlay contains no evidence of a dormant ISO scheduler or HCI command
path. LE Audio also requires ISO HCI transport packets, controller scheduling,
data paths, and host stack support; advertising ISO bits alone is insufficient.

## Hardware completion update (2026-10-13)

The original `851968`-byte capture with SHA-256
`eacbd0114a542675a817f67598ba400c84e2194133b0237c1691421431ac5726` and
CRC32 `0xc8105e31` is invalid despite passing chunk CRCs. The original RAM
capture is also invalid. Use corrected `work/cyw43439-btrom-clean.bin`
(`589824` bytes, SHA-256
`d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038`).
Stock Pico SDK `.0065` and direct opcode probes found no hidden ISO
HCI handlers. Newer `.0071` patch tests fail Pico board initialization and do not
provide a drop-in upgrade. Detailed results: `analysis/hardware-results.md`.
