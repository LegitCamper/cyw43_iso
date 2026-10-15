# Marker-acquired timer-driven diagnostic RX hopping

`--timer-hop` builds a separate 2048-byte diagnostic image. Existing fixed-channel,
stream, and packet-driven hopping images remain byte-identical.

## Mechanism

The stock DTM callback table slot `+0x24` points at ROM `0x713fc`. Scheduler
paths dispatch that slot for the active state-2 event independently of receive
completion. In RX mode the stock callback returns after its event/pending work;
its transmit countdown path is skipped. The timer-hop phase wrapper replaces
only the resident clone's slot `+0x24`, after calling stock phase setup.

The replacement callback always calls stock first and validates event
`0x209788`, parameters `0x2097b8`, and RX mode. Before marker synchronization,
every accepted callback advances the 16-bit candidate event counter once and
retunes to its CSA#2 channel. Existing static timing evidence associates these
callbacks with nominal 625-us ticks. This fast diagnostic scan tries 32 CSA#2
event candidates during one nominal 20-ms transmitter interval instead of
waiting 20 ms on one wrong channel.

The first accepted 26-byte diagnostic counter marker can acquire the scan:
bytes 2..25 must equal `02..19`, while bytes 0..1 provide the little-endian
16-bit event index. The RX hook copies the payload, calls stock first, then
atomically sets the event counter to that index, configures its CSA#2 channel,
resets the countdown to 32, and publishes the synchronization word. The
acquisition channel configuration increments the common configuration count.
Later packets cannot reacquire or move the phase.

After synchronization, callbacks stop scanning. They decrement the 32-tick
countdown and advance exactly one CSA#2 event at zero, preserving the marker
synchronization word and scan count across 16-bit event-counter wrap. Receive
callbacks never perform a post-acquisition event advance.

This is marker-fixture acquisition, not BIGInfo-derived anchor synchronization.
It assumes the received marker index identifies the current transmitter event
and starts the next nominal interval at RX completion. It does not recover a
Bluetooth broadcast anchor, measure controller timer phase, or parse BIGInfo.
The pre-acquisition scan is not Bluetooth standard discovery and does not
guarantee that every logical channel is visited during one transmitter event.

The eight `u32` trace fields at resident offset 1100 are event counter, current
channel, channel configuration count, ticks remaining, accepted countdown
callbacks, synchronized event advances, marker synchronized (`0` or `1`), and
pre-acquisition scan retunes. Configuration count includes initial setup, every
scan retune, marker acquisition retune, and every synchronized event retune.

## Checks

```sh
uv run --quiet --with unicorn python analysis/check-controller-rx.py \
  --bis --iso --stream --timer-hop --emulate \
  --output-blob work/iso-blob-rx-timer-hop
```

Unicorn checks stock countdown chaining, callback guards, one CSA#2 scan retune
per accepted unsynchronized tick, rejection of a non-marker payload, one-shot
acquisition from a valid marker, acquisition configuration accounting, 32-tick
phase reset, interrupt-mask restoration, no RX-driven advance, exactly one
post-acquisition advance per 32 accepted ticks, and synchronized 16-bit counter
wrap without clearing synchronization or scan telemetry. Phase emulation checks
event-zero CSA#2 configuration and installation of the resident countdown
callback.

Generated image SHA-256 is
`5657f8bad24d5987d67788d46d79173b35a8f0ffbf84c69b9be34edbf0d43090`.
Allocator SHA-256 is
`dc1c2dfabb13f4a8de26ab8328de4def56944d2d646bbe65cf201295bdbc4d47`.

## Live gate

No scan-enabled hardware run is claimed. Run silent, correctly aligned hopping,
and one-event-offset controls, with stock firmware restoration checks after each
run. Confirm scan telemetry grows before acquisition, synchronization changes to
one only after a delivered marker, and synchronized advances then follow the
32-tick cadence. Sustained reception after a loss can validate this marker
diagnostic, but only BIGInfo-derived anchor and interval handling can promote it
to broadcast synchronization.
