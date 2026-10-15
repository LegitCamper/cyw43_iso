# Timer-driven diagnostic RX hopping

`--timer-hop` builds a separate 2048-byte diagnostic image. Existing fixed-channel,
stream, and packet-driven hopping images remain byte-identical.

## Mechanism

The stock DTM callback table slot `+0x24` points at ROM `0x713fc`. Scheduler
paths dispatch that slot for the active state-2 event independently of receive
completion. In RX mode the stock callback returns after its event/pending work;
its transmit countdown path is skipped. The timer-hop phase wrapper replaces
only the resident clone's slot `+0x24`, after calling stock phase setup.

The replacement callback calls stock first, validates event `0x209788`,
parameters `0x2097b8`, and RX mode, then advances one CSA#2 event every 32
callbacks. Existing static timing evidence associates this scheduler countdown
with nominal 625-us ticks, making 32 ticks nominally 20 ms. This image does not
claim measured tick spacing or a BIGInfo-derived anchor.

Receive callbacks never advance the event counter in this mode. Lost or
CRC-rejected packets therefore do not stall hopping. Trace fields are event
counter, current channel, channel configuration count, ticks remaining,
accepted countdown callbacks, and completed event advances.

## Checks

```sh
uv run --quiet --with unicorn python analysis/check-controller-rx.py \
  --bis --iso --stream --timer-hop --emulate \
  --output-blob work/iso-blob-rx-timer-hop
```

Unicorn checks stock countdown chaining, callback guards, exactly one advance
per 32 accepted ticks, 64 timer ticks with no RX callback, 16-bit event-counter
wrap, and no event advance from RX callbacks. Phase emulation checks event-zero
CSA#2 configuration and installation of the resident countdown callback.

Generated image SHA-256 is
`60f0e127a36b0d0a04c4ee0860280ce2f170344a46f24097acac9c11744bdf5a`.
Allocator SHA-256 is
`dc1c2dfabb13f4a8de26ab8328de4def56944d2d646bbe65cf201295bdbc4d47`.

## Live gate

No hardware run is claimed. Before promotion, measure countdown callback cadence
and verify RF retuning occurs at a safe boundary while reception is active.
Then run silent, correctly aligned hopping, and one-event-offset controls, with
stock firmware restoration checks after each run.
