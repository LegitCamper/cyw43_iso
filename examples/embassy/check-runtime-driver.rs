//! Offline actual-driver-seam checks. Run command documented in README.md.
#![allow(dead_code, private_bounds)]
extern crate self as embassy_time;
extern crate self as embedded_io_async;
use std::{
    collections::BTreeMap,
    future::Future,
    marker::PhantomData,
    pin::pin,
    task::{Context, Poll, Waker},
};

#[derive(Debug, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidInput,
    InvalidData,
    NotConnected,
    TimedOut,
}
pub struct Duration;
impl Duration {
    fn from_millis(_: u64) -> Self {
        Self
    }
}
pub struct Timer;
impl Timer {
    async fn after_millis(_: u64) {}
}
pub async fn with_timeout<F: Future>(_: Duration, future: F) -> Result<F::Output, ()> {
    Ok(future.await)
}

const BTSDIO_FWBUF_SIZE: u32 = 4096;
const BTSDIO_OFFSET_HOST_WRITE_BUF: u32 = 0;
const BTSDIO_OFFSET_HOST_READ_BUF: u32 = 4096;
const BTSDIO_OFFSET_HOST2BT_IN: u32 = 8192;
const BTSDIO_OFFSET_HOST2BT_OUT: u32 = 8196;
const BTSDIO_OFFSET_BT2HOST_IN: u32 = 8200;
const BTSDIO_OFFSET_BT2HOST_OUT: u32 = 8204;
const BASE: u32 = 0x19400000;

trait Bus {
    async fn bp_read(&mut self, at: u32, bytes: &mut [u8]);
    async fn bp_write(&mut self, at: u32, bytes: &[u8]);
    async fn bp_read32(&mut self, at: u32) -> u32;
    async fn bp_write32(&mut self, at: u32, value: u32);
}
struct Channel {
    queued: bool,
}
impl Channel {
    fn try_receive(&mut self) -> Option<()> {
        self.queued.then_some(())
    }
}
struct Packet {
    buf: [u8; 1024],
    len: usize,
}
struct RxChannel {
    packet: Packet,
    full: bool,
}
impl RxChannel {
    fn try_send(&mut self) -> Option<&mut Packet> {
        (!self.full).then_some(&mut self.packet)
    }
    fn send_done(&mut self) {
        self.full = true;
    }
}
struct BtRunner {
    addr: u32,
    h2b_write_pointer: u32,
    b2h_read_pointer: u32,
    tx_chan: Channel,
    rx_chan: RxChannel,
}
impl BtRunner {
    async fn bt_bus_request(&mut self, _: &mut impl Bus) {}
    async fn bt_toggle_intr(&mut self, _: &mut impl Bus) {}
    async fn hci_write(&mut self, _: &mut impl Bus) {
        self.tx_chan.queued = false;
    }
}
pub struct Runner<'a, BUS> {
    bus: BUS,
    bt: Option<BtRunner>,
    lifetime: PhantomData<&'a ()>,
}
include!("driver_runtime.rs");

#[derive(Default)]
struct FakeBus {
    bytes: BTreeMap<u32, u8>,
    writes: Vec<(u32, Vec<u8>)>,
    reply: Vec<u8>,
    response_start: u32,
    drop_writes: bool,
}
impl FakeBus {
    fn store(&mut self, at: u32, bytes: &[u8]) {
        for (i, byte) in bytes.iter().enumerate() {
            self.bytes.insert(at + i as u32, *byte);
        }
    }
    fn load(&self, at: u32, count: usize) -> Vec<u8> {
        (0..count)
            .map(|i| *self.bytes.get(&(at + i as u32)).unwrap_or(&0))
            .collect()
    }
    fn ring(&self, base: u32, start: u32, count: usize) -> Vec<u8> {
        (0..count)
            .map(|i| self.load(base + (start + i as u32) % 4096, 1)[0])
            .collect()
    }
}
impl Bus for FakeBus {
    async fn bp_read(&mut self, at: u32, bytes: &mut [u8]) {
        assert_eq!(at % 4, 0);
        bytes.copy_from_slice(&self.load(at, bytes.len()));
    }
    async fn bp_write(&mut self, at: u32, bytes: &[u8]) {
        assert_eq!(at % 4, 0);
        assert_eq!(at % 4, 0);
        assert_eq!(bytes.len() % 4, 0);
        self.writes.push((at, bytes.to_vec()));
        if !self.drop_writes {
            self.store(at, bytes);
        }
    }
    async fn bp_read32(&mut self, at: u32) -> u32 {
        u32::from_le_bytes(self.load(at, 4).try_into().unwrap())
    }
    async fn bp_write32(&mut self, at: u32, value: u32) {
        assert_eq!(at % 4, 0);
        self.writes.push((at, value.to_le_bytes().to_vec()));
        if !self.drop_writes {
            self.store(at, &value.to_le_bytes());
        }
        if at == BASE + BTSDIO_OFFSET_HOST2BT_IN && !self.reply.is_empty() {
            let mut packet = vec![self.reply.len() as u8, (self.reply.len() >> 8) as u8, 0, 4];
            packet.extend_from_slice(&self.reply);
            packet.resize(packet.len().div_ceil(4) * 4, 0);
            for (i, byte) in packet.iter().enumerate() {
                self.store(
                    BASE + 4096 + (self.response_start + i as u32) % 4096,
                    &[*byte],
                );
            }
            self.store(
                BASE + BTSDIO_OFFSET_BT2HOST_IN,
                &((self.response_start + packet.len() as u32) % 4096).to_le_bytes(),
            );
            self.store(BASE + BTSDIO_OFFSET_HOST2BT_OUT, &value.to_le_bytes());
        }
    }
}
fn ready<F: Future>(future: F) -> F::Output {
    match pin!(future)
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("mock unexpectedly blocked"),
    }
}
fn setup(tx: u32, rx: u32, response: Vec<u8>) -> Runner<'static, FakeBus> {
    let mut bus = FakeBus {
        reply: response,
        response_start: rx,
        ..FakeBus::default()
    };
    bus.store(BASE + BTSDIO_OFFSET_HOST2BT_OUT, &tx.to_le_bytes());
    bus.store(BASE + BTSDIO_OFFSET_BT2HOST_IN, &rx.to_le_bytes());
    Runner {
        bus,
        bt: Some(BtRunner {
            addr: BASE,
            h2b_write_pointer: tx,
            b2h_read_pointer: rx,
            tx_chan: Channel { queued: false },
            rx_chan: RxChannel {
                packet: Packet {
                    buf: [0; 1024],
                    len: 0,
                },
                full: false,
            },
        }),
        lifetime: PhantomData,
    }
}

#[test]
fn command_wraps_both_rings_and_publishes_after_payload() {
    for tx in (4080..4096).step_by(4) {
        for rx in (4080..4096).step_by(4) {
            let event = vec![
                0x0e, 12, 1, 1, 0x10, 0, 0x0b, 0, 0, 0x0b, 0x0f, 0, 0x10, 0x23,
            ];
            let mut runner = setup(tx, rx, event.clone());
            let mut response = [0; 260];
            assert_eq!(
                ready(runner.bluetooth_runtime_command(&[1, 0x10, 0], &mut response)),
                Ok(14)
            );
            assert_eq!(&response[..14], &event);
            assert_eq!(runner.bus.ring(BASE, tx, 8), [3, 0, 0, 1, 1, 0x10, 0, 0]);
            let publications: Vec<_> = runner
                .bus
                .writes
                .iter()
                .enumerate()
                .filter(|(_, (at, _))| *at == BASE + 8192)
                .collect();
            assert_eq!(publications.len(), 1);
            assert!(publications[0].0 > 0);
            assert_eq!(
                runner.bt.as_ref().unwrap().b2h_read_pointer,
                (rx + 20) % 4096
            );
        }
    }
}

#[test]
fn rejects_bad_packets_credits_busy_rings_and_timeout() {
    for base in [0, BASE | 1, u32::MAX & !3] {
        let mut runner = setup(0, 0, vec![]);
        runner.bt.as_mut().unwrap().addr = base;
        assert_eq!(
            ready(runner.bluetooth_runtime_command(&[1, 0x10, 0], &mut [0; 260])),
            Err(ErrorKind::InvalidData)
        );
        assert_eq!(ready(runner.bluetooth_runtime_poll()), Err(ErrorKind::InvalidData));
        assert!(runner.bus.writes.is_empty());
    }
    for event in [
        vec![0x0e, 4, 1, 2, 0x10, 0],
        vec![0x0e, 4, 0, 1, 0x10, 0],
        vec![0x0e, 5, 1, 1, 0x10, 0],
    ] {
        let mut runner = setup(0, 0, event);
        assert_eq!(
            ready(runner.bluetooth_runtime_command(&[1, 0x10, 0], &mut [0; 260])),
            Err(ErrorKind::InvalidData)
        );
        assert_eq!(runner.bt.as_ref().unwrap().b2h_read_pointer, 0);
    }
    let mut runner = setup(0, 0, vec![]);
    assert_eq!(
        ready(runner.bluetooth_runtime_command(&[1, 0x10, 1], &mut [0; 260])),
        Err(ErrorKind::InvalidInput)
    );
    assert!(runner.bus.writes.is_empty());
    runner.bus.store(BASE + 8196, &4u32.to_le_bytes());
    assert_eq!(
        ready(runner.bluetooth_runtime_command(&[1, 0x10, 0], &mut [0; 260])),
        Err(ErrorKind::InvalidData)
    );
    assert!(runner.bus.writes.is_empty());
    runner.bus.store(BASE + 8196, &0u32.to_le_bytes());
    assert_eq!(
        ready(runner.bluetooth_runtime_command(&[1, 0x10, 0], &mut [0; 260])),
        Err(ErrorKind::TimedOut)
    );
}

#[test]
fn memory_writes_require_readback_and_stay_in_allowed_ranges() {
    let mut runner = setup(0, 0, vec![]);
    assert_eq!(
        ready(runner.bluetooth_runtime_write(0x203a34, &[1, 2, 3, 4])),
        Ok(())
    );
    let mut bytes = [0; 4];
    assert_eq!(
        ready(runner.bluetooth_runtime_read(0x203a34, &mut bytes)),
        Ok(())
    );
    assert_eq!(bytes, [1, 2, 3, 4]);
    runner.bus.drop_writes = true;
    assert_eq!(
        ready(runner.bluetooth_runtime_write(0x203a34, &[4, 3, 2, 1])),
        Err(ErrorKind::InvalidData)
    );
    let count = runner.bus.writes.len();
    assert_eq!(
        ready(runner.bluetooth_runtime_write(0x318b70, &[0; 4])),
        Err(ErrorKind::InvalidInput)
    );
    assert_eq!(runner.bus.writes.len(), count);
}

#[test]
fn pump_receives_one_wrapped_credit_without_blocking_on_channel() {
    let mut runner = setup(0, 4092, vec![]);
    runner.bus.store(BASE + 4096 + 4092, &[7, 0, 0, 4]);
    runner
        .bus
        .store(BASE + 4096, &[0x13, 5, 1, 0x42, 0, 1, 0, 0]);
    runner.bus.store(BASE + 8200, &8u32.to_le_bytes());
    assert_eq!(ready(runner.bluetooth_runtime_poll()), Ok(true));
    let bt = runner.bt.as_ref().unwrap();
    assert_eq!(&bt.rx_chan.packet.buf[..8], &[4, 0x13, 5, 1, 0x42, 0, 1, 0]);
    assert_eq!(bt.rx_chan.packet.len, 8);
    assert_eq!(bt.b2h_read_pointer, 8);
    runner.bus.store(BASE + 4096 + 8, &[7, 0, 0, 4]);
    runner.bus.store(BASE + 8200, &20u32.to_le_bytes());
    assert_eq!(
        ready(runner.bluetooth_runtime_poll()),
        Err(ErrorKind::InvalidData)
    );
    assert_eq!(runner.bt.as_ref().unwrap().b2h_read_pointer, 8);
}
