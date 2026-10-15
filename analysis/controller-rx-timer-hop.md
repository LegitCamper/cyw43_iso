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
every accepted callback advances the 16-bit candidate event counter once,
retunes to its CSA#2 channel, and then re-executes the stock RX arming
sequence at ROM `0x714a4` so the receiver restarts on the new channel.
Existing static timing evidence associates these callbacks with nominal
625-us ticks. This fast diagnostic scan tries 32 CSA#2
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

Every accepted callback first requires event byte `+0x0f` to equal 1, the same
currency condition stock RX processing uses before arming. A tick that fails it
increments the accepted-callback count and does nothing else: no counter
advance, no countdown change, no channel write, and no rearm. The receiver is
therefore never reprogrammed while the event is not current.

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
Because non-current ticks are counted but perform no work, accepted callbacks
are an upper bound on scan plus synchronized ticks rather than an exact sum.

## Checks

```sh
uv run --quiet --with unicorn python analysis/check-controller-rx.py \
  --bis --iso --stream --timer-hop --emulate \
  --output-blob work/iso-blob-rx-timer-hop
```

Unicorn checks stock countdown chaining, callback guards, one CSA#2 scan retune
plus one stock RX rearm per accepted unsynchronized tick, counted-only ticks
that change nothing else while the event is not current, rejection of a non-marker payload, one-shot
acquisition from a valid marker, acquisition configuration accounting, 32-tick
phase reset, interrupt-mask restoration, no RX-driven advance, exactly one
post-acquisition advance per 32 accepted ticks, and synchronized 16-bit counter
wrap without clearing synchronization or scan telemetry. Phase emulation checks
event-zero CSA#2 configuration and installation of the resident countdown
callback.

Generated image SHA-256 is
`651b509797ae3907976e295233b5ecc5473268df36990b29d084a0fc5951461b`.
Allocator SHA-256 is
`dc1c2dfabb13f4a8de26ab8328de4def56944d2d646bbe65cf201295bdbc4d47`.

## Live result: timer retuning suppresses reception

Three live Pico W runs with the scan image delivered no packets at all, and the
decisive one removes channel alignment as an explanation.

| Run | Transmitter | Delivered | Scan retunes | Marker |
| --- | --- | --- | --- | --- |
| Silent | none | 0 | 32036 | 0 |
| Aligned CSA#2 marker | `marker-counter` hopping | 0 | 32035 | 0 |
| Fixed channel 0 | `marker-counter`, 2404 MHz, no hopping | 0 | 32036 | 0 |

The fixed-channel transmitter completed `DTM_TX done sent=500 disabled=true`
while scanning across all CSA#2 candidates. The same transmitter and the same
host deliver 500 of 500 with the non-timer `--stream` image, so the radio, the
marker fixture, the mailbox, and the host path are all known good. Scanning
therefore visits the transmitter's channel many times per packet yet still
receives nothing.

This is strong evidence that retuning from the countdown callback leaves the
receiver unarmed: the stock RX rearm path and an asynchronous channel
reconfiguration from the timer context are not compatible in this image. The
earlier packet-driven `--hop` image retuned only after a stock receive callback
had rearmed the radio, and it did receive consecutive events. Static emulation
cannot see this because it stubs the radio helpers.

Trace accounting was exact in every run: `A = S`, `V = 0`, `T = 32`,
`Q = 1 + S`, and the reported channel equalled CSA#2 of the reported counter.
The defect is in radio arming, not in the counter, scan, or trace logic.

### Root cause and the rearm fix

Static ROM evidence explains the result exactly. Stock countdown `0x713fc`
returns immediately in RX mode: at `0x71416..0x7141e` it loads parameters `+6`
and pops when that byte is nonzero. Its countdown decrement and the
`0x318014` self-write repeat trigger are TX-only. All RX arming lives in
receive processing `0x7143e`, at `0x714a4..0x714ca`: it clears bit 7 of
`0x318014`, runs stock settle delay `0x1711a(2)`, sets `0x2000000` in
`0x31801c`, and writes bit 7 of `0x318014` back. Retuning from the countdown
callback therefore changed channel registers without any arming, which matches
zero deliveries across every scan candidate.

The image now calls that exact ROM arming sequence, reusing the code at
`0x714a5` with the event in `r4`, immediately after each timer-context retune.
Nothing is reimplemented and no new MMIO write is introduced; the same three
register operations stock performs after a receive now also follow a timer
retune. Live validation of this version is still outstanding, and only
BIGInfo-derived anchor and interval handling can promote marker acquisition to
broadcast synchronization.

Hardware restoration: each run passed two stock capability checks with LMP
`0x2310`, and the marker and fixed-channel runs restored Nordic RRAM
`434bbc66505f44ff1e5d50693205238fb17f7791785611df6e3e8148021a2573` and UICR
`06f4af83fcd2659a91ad36d1bc18aa0504ef4f890668b4ba50e7f2587779ee28`
byte-identically. Evidence:
`work/embassy-normal-rx-timer-hop-series-v6/`.
