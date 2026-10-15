# Configuration signature rejection

The `.0065` configuration mutations invalidate an ECDSA P-256 signature.
This resolves the configuration-validation gate: the name-only control reaches
the enabled ROM signature validator with correctly downloaded bytes and an
invalid signature. The hook-table redirect also fails offline verification
against the same public key. No further hook image was flashed for this analysis.

## ROM validation path

Source is the corrected, post-load ROM capture
`work/cyw43439-btrom-clean.bin`, SHA-256
`d5e271f20c80293be3561bb63e92ce4f6ca563ecca8fffd81eba639de9354038`.
Focused Ghidra disassembly and pseudocode are retained in
`work/ghidra/inspect_config_validation.txt`,
`work/ghidra/inspect_config_crypto.txt`, and
`work/ghidra/inspect_config_hash_core.txt`. These are local static-analysis
artifacts, not REA Evidence records or execution traces. Assembly resolves
incorrect argument counts in the inferred pseudocode.

| Address | Observed behavior |
| --- | --- |
| `0x0000507a` | Processes static configuration, then checks dynamic configuration before applying its records |
| `0x00005098` | Reads validation-enable byte at `0x00200681` |
| `0x0000509c` | Calls validator `0x00006a78` when validation is enabled |
| `0x00006a94` | Loads dynamic payload length from header offset `12`, then adds the 16-byte header |
| `0x00006a9e` | Hashes this entire range through `0x0003eaba` |
| `0x00006ab4` | Reads the following 64 bytes into signature scratch at `0x00209bac` |
| `0x00006ad8` | Rejects an all-zero signature |
| `0x00006ae4` | Loads public-key source pointer from `0x00200688`, copies its 64 bytes, and counts zero bits |
| `0x00006b32` | Compares that count with the halfword following the key |
| `0x00006b4e` | Calls ECDSA verifier `0x00018f24(digest, signature, public_key)` |
| `0x000050a0` | A zero validator result branches to failure handling and infinite loop at `0x000050c4` |
| `0x000050a6` | Only after successful validation, processes dynamic configuration records |
| `0x000050b4` | Writes configuration-complete byte at `0x00200682` on success |

The host SDK does not validate configuration names, checksums, or signatures.
`cybt_shared_bus.c:152` returns `CYBT_ERR_HCI_INIT_FAILED` (`180`) when
`cybt_wait_bt_ready` times out waiting for bit 24 of backplane register
`0x18000c7c`. The new name-control capture includes its exact branch log:
`[CYW43] hci_open(): Failed to download FW`.
The other `180` branch at line 142 is a downloader argument failure and is not
the branch observed in this capture.

## Signed range and byte order

For stock `.0065`, `BRCMcfgD` starts at `0x0021f000`. The header declares
`0x0c30` payload bytes. The signed input is consequently `0x0c40` bytes,
`[0x0021f000, 0x0021fc40)`. The 64-byte signature occupies
`[0x0021fc40, 0x0021fc80)`.

Both the name byte at `0x0021f029` and the compact hook record at
`0x0021f918` fall inside the signed range. The append-only control preserves
this range and its signature; the redirect changes the signed hook pointer.
The container version string's `_dl_signed` suffix is not needed for this
conclusion.

The digest is **not ordinary SHA-256 of the configuration file**:

1. Initialize the standard SHA-256 state.
2. Reverse all `0x0c40` input bytes, then compress the resulting 49 blocks.
3. Do not add SHA-256 padding. For this block-aligned length, ROM finalization
   at `0x0003eb18` skips padding because its buffered-byte count is zero.
4. Serialize the eight state words in standard digest order, then reverse
   all 32 output bytes. These bytes are passed to the ECDSA verifier.

This recipe is scoped to the observed block-aligned image. Non-aligned images
take a separate ROM padding path and are not covered by the check.

Signature `r` and `s` are two 32-byte big-endian integers. Public-key `x` and
`y` are two 32-byte little-endian integers. The live key pointer is
`0x002004ee`; its trailing integrity halfword is `248`, equal to the count of
zero bits in the 64 key bytes. The same key and integrity value appear in both
stock and failing captures. P-256 parameters are independently visible in the
captured curve table at `0x00202784`.

## Live controls and offline verification

`hardware/picow_config_probe.c` enables SDK error logging and performs only
bounded, checked backplane reads after initialization returns. It reports
diagnostic RAM even on failure; those bytes are not hook-entry telemetry.
Each retained capture contains two complete passes.

| Check | Stock | Name-only `RPI` to `RPJ` |
| --- | --- | --- |
| Initialization result | `0` | `180` |
| Raw ready register | `0x01000100` | `0x00000000` |
| Validation enabled, `0x00200681` | `1` | `1` |
| Configuration complete, `0x00200682` | `1` | `0` |
| Post-record validation flag, `0x00200684` | `1` | `1` |
| Full dynamic-config readback | Matches stock exactly | Matches intended one-byte mutation exactly |
| Offline ECDSA result | Valid | Invalid |

The name-control digest retained at `0x002006f4` is
`a5bf22159a24c1de16798055f05d3a2777b1eb4062a3da87b593b479171a3494`.
It matches the offline computation exactly in both passes. In the first failed
pass, signature scratch matches the unchanged trailer, and key scratch matches
the validated source key. Those scratch regions are reused before the second
pass; they must not be interpreted as persistent validation outputs. This
excludes a download-byte mismatch, all-zero trailer, or key-integrity failure
for the captured control.

Stock's full-configuration digest is
`aebbd14879100d93214ebdf90be23c4bd8856df7b3dab86bb8576b0d5ba1dced`.
Do not compare it with stock's final digest scratch: later record validation
through `0x00006b60` reuses that buffer. Successful startup also reuses the
signature scratch for other state.

The offline check verifies stock and append-only configurations and rejects
both the name mutation and hook redirect with their unchanged signature.
Run from the repository root with the existing OpenSSL and Python
`cryptography` installations (no new dependencies were installed):

```sh
python3 analysis/check-config-validation.py
```

Evidence captures:

| Artifact | SHA-256 |
| --- | --- |
| `work/config-validation/stock.txt` | `cdc31bd3caa5bed071e3ff19eac1ae016202b3c22ed0eb15e1bff8612c8f9273` |
| `work/config-validation/config-name.txt` | `8e32549d0633008684b7f28e4e3258396a96daeae4747110252e78d4450933d5` |
| `work/config-validation/stock-restored.txt` | `4eab5268f1f73c24986328ba61a2d091bf308a0ce7cf755a1969d98dd8b1e78d` |

Build logs, SWD programming/verification logs, and capture helper reside in
`work/config-validation/`. Stock HCI probe was restored through SWD with
`Verified OK`; two subsequent live results report LMP subversion `0x2310`,
LE features `ff40010c00000000`, and no ISO support.

## Limits and next gate

The live name-control failure is attributed to signature validation using
static control flow, exact RAM readback, and independent cryptographic
verification. No Bluetooth-core PC trace was captured. The redirect's invalid
signature is independently proven offline; its failure-state RAM was not
recaptured in this pass. Earlier redirect error `180` remains the live result.

The signed hook table cannot be changed while retaining a valid stock
signature. Post-boot instrumentation now executes through a verified RAM
callback replacement after booting an image with the unchanged signed config;
see `analysis/scheduler-hooks.md`. This proves one command-lookup wrapper, not
ISO scheduling or arbitrary boot-time code changes. ROM also performs the
code-region comparison through `0x00006b60` described below.

## Stock code-region descriptors

`analysis/check-code-validation.py` reconstructs stock memory using the existing
Rust container parser and checks the 16 consecutive `0x0133` records at config
offsets `0x982..0xc32`. Each has a 40-byte payload: little-endian address and
length followed by a 32-byte digest. Their nonoverlapping ranges cover exactly:

| Range | Bytes | Descriptor count |
| --- | ---: | ---: |
| `[0x000d0200, 0x000d0f90)` | 3472 | 14 |
| `[0x002038d4, 0x002038d8)` | 4 | 1 |
| `[0x00204164, 0x00204168)` | 4 | 1 |

The 14 code descriptors comprise thirteen 251-byte regions and one 209-byte
region, in a non-address-sorted order. The final descriptor, at config offset
`0xc07`, describes `[0x000d06e7, 0x000d07e2)`.

Every stored descriptor digest equals `SHA256(region_bytes[::-1])`. These
lengths are not block-aligned and all have bit lengths below 65536, so the ROM
padding path is equivalent to standard SHA-256 padding for these inputs.
The ROM output reverses those 32 digest bytes, as established for the config
hash. Both retained stock passes leave exactly that reversed output for the
final descriptor in digest scratch `0x002006f4`:

```text
stored: 6b15318c52d3d48f41f62fc2043f8b97c361e0d92cb0ada21daac0670c2c9e5c
scratch: 5c9e2c0c67c0aa1da2adb02cd9e061c3978b3f04c22ff6418fd4d3528c31156b
```

The comparison is unusual and must not be summarized as ordinary digest
equality validation. At `0x00006bb4`, ROM calls byte comparator `0x00081ed8`;
that function returns zero for equality. The `bne` at `0x00006bba` preserves
flag `0x00200684` on inequality; equality falls through and clears it at
`0x00006bbc`. The enabled caller at `0x000050ae` treats a cleared flag as
failure. Raw Thumb instructions confirm this control flow independently of
Ghidra's inferred pseudocode.

There is no descriptor byte reversal in the retained stock reader path:
live word `0x0020062c` holds Thumb pointer `0x00004c63`; `0x00004c62` selects
the config base (`0x0021f000` for selector 1), adds the offset, and tail-calls
`0x00081f6e` to copy bytes. SHA finalization at `0x0003eb92` also copies its
state bytes directly. Thus the observed stock mismatch agrees with both the
success flag and the successful startup; it is not merely a display-endian
difference. Relevant ROM paths are retained in
`work/ghidra/inspect_config_hash_core.txt` and
`work/ghidra/inspect_config_validation.txt`; other cited instructions can be
rechecked directly in the SHA-256-identified clean ROM.

Run `python3 analysis/check-code-validation.py` to verify descriptor coverage,
all stock digests, reader state, and both live final-scratch matches. The check
is deliberately scoped to the `.0065` descriptor table and stock captures.
It does not test a modified code image or prove that arbitrary code changes
boot successfully.
