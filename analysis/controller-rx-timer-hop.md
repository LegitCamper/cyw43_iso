# Marker-acquired timer-driven diagnostic RX hopping

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
with nominal 625-us ticks, making 32 ticks nominally 20 ms. Receive callbacks
never advance this timer counter.

Before synchronization, the timer still advances open loop. The first accepted
26-byte diagnostic counter marker can acquire it: bytes 2..25 must equal
`02..19`, while bytes 0..1 provide the little-endian 16-bit event index. The RX
hook copies the payload, calls stock first, then atomically sets the event
counter to that index, configures its CSA#2 channel, resets the countdown to 32,
and publishes the synchronization word. Later packets cannot reacquire or move
the phase. Timer advances retain that word across 16-bit event-counter wrap.

This is marker-fixture acquisition, not BIGInfo-derived anchor synchronization.
It assumes the received marker index identifies the current transmitter event
and starts the next nominal interval at RX completion. It does not recover a
Bluetooth broadcast anchor, measure controller timer phase, or parse BIGInfo.

The seven `u32` trace fields at resident offset 1096 are event counter, current
channel, channel configuration count, ticks remaining, accepted countdown
callbacks, completed event advances, and marker synchronized (`0` or `1`).

## Checks

```sh
uv run --quiet --with unicorn python analysis/check-controller-rx.py \
  --bis --iso --stream --timer-hop --emulate \
  --output-blob work/iso-blob-rx-timer-hop
```

Unicorn checks stock countdown chaining, callback guards, exactly one advance
per 32 accepted ticks, timer progress with no RX callback, rejection of a
non-marker payload, one-shot acquisition from a valid marker, 32-tick phase
reset, interrupt-mask restoration, no RX-driven advance, and synchronized
16-bit event-counter wrap. Phase emulation checks event-zero CSA#2
configuration and installation of the resident countdown callback.

Generated image SHA-256 is
`db0bb61072703378ce944975ab092c1b8ee34c1a3adf80e20aa9b46410bede82`.
Allocator SHA-256 is
`dc1c2dfabb13f4a8de26ab8328de4def56944d2d646bbe65cf201295bdbc4d47`.

## Live gate

No hardware run is claimed. Before promotion, measure countdown callback cadence
and verify RF retuning occurs at a safe boundary while reception is active.
Then run silent, correctly aligned hopping, and one-event-offset controls, with
stock firmware restoration checks after each run. Sustained reception after a
loss can validate this marker diagnostic, but only BIGInfo-derived anchor and
interval handling can promote it to broadcast synchronization.
