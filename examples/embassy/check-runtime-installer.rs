//! Run the actual installer against a deterministic controller-memory model.
#![allow(dead_code)]
extern crate self as cyw43;
extern crate self as embedded_hal;
extern crate self as embassy_time;
use std::{collections::BTreeMap, future::Future, marker::PhantomData, pin::pin, task::{Context, Poll, Waker}};
pub mod digital { pub trait OutputPin {} }
pub trait SpiBusCyw43 {}
pub struct SpiBus<P, S>(PhantomData<(P, S)>);
pub struct Timer;
impl Timer { pub async fn after_millis(_: u64) {} }
struct Pin;
impl digital::OutputPin for Pin {}
struct Spi;
impl SpiBusCyw43 for Spi {}
pub struct Runner<'d, B> {
    memory: BTreeMap<u32, u8>,
    writes: usize,
    fail_write: Option<usize>,
    marker: PhantomData<(&'d (), B)>,
}
const ALLOCATION: u32 = 0x217e78;
const TRANSPORT: u32 = 0x207100;
impl<B> Runner<'_, B> {
    fn bytes(&self, address: u32, length: usize) -> Vec<u8> {
        (0..length).map(|i| *self.memory.get(&(address + i as u32)).unwrap_or(&0)).collect()
    }
    fn word(&self, address: u32) -> u32 { u32::from_le_bytes(self.bytes(address, 4).try_into().unwrap()) }
    fn store(&mut self, address: u32, bytes: &[u8]) {
        for (i, byte) in bytes.iter().enumerate() { self.memory.insert(address + i as u32, *byte); }
    }
    fn put(&mut self, address: u32, value: u32) { self.store(address, &value.to_le_bytes()); }
    pub async fn bluetooth_runtime_read(&mut self, address: u32, bytes: &mut [u8]) -> Result<(), ()> {
        bytes.copy_from_slice(&self.bytes(address, bytes.len())); Ok(())
    }
    pub async fn bluetooth_runtime_write(&mut self, address: u32, bytes: &[u8]) -> Result<(), ()> {
        self.writes += 1;
        self.store(address, bytes);
        if self.fail_write == Some(self.writes) {
            self.fail_write = None;
            return Err(()); // Ambiguous failure: the write reached hardware.
        }
        Ok(())
    }
    pub async fn bluetooth_runtime_command(&mut self, command: &[u8], response: &mut [u8; 260]) -> Result<usize, ()> {
        let opcode = u16::from_le_bytes(command[..2].try_into().unwrap());
        let parameters = match opcode {
            0x0c03 => vec![0],
            0x1001 => {
                if self.word(0x203a34) == 0xd0f91 && self.word(0xd0ff0) == 0 {
                    assert_eq!(self.bytes(0xd0fc8, 2), [0x80, 0x50]);
                    assert_eq!(self.word(0xd0ff4), ALLOCATION);
                    assert_eq!(self.word(0xd0ff8), 0x220000);
                    self.put(0x200564, ALLOCATION + 4096);
                    self.put(0xd0ff0, ALLOCATION);
                }
                vec![0, 0x0b, 0, 0, 0x0b, 0x31, 1, 0x10, 0x23]
            }
            0x2002 => vec![0, 251, 0, 4],
            0x2060 => {
                assert_eq!(self.word(0x203a34), ALLOCATION + 3073);
                vec![0, 251, 0, 4, 30, 0, 4]
            }
            0x201e => { self.put(0x209788, 1); vec![0] }
            0x201f => { self.put(0x209788, 0); vec![0, 20, 0] }
            _ => panic!("unexpected command"),
        };
        response[..5].copy_from_slice(&[14, (parameters.len() + 3) as u8, 1, command[0], command[1]]);
        response[5..5 + parameters.len()].copy_from_slice(&parameters);
        Ok(parameters.len() + 5)
    }
}
#[path = "runtime.rs"] mod runtime;

fn ready<F: Future>(future: F) -> F::Output {
    match pin!(future).as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(value) => value, Poll::Pending => panic!("mock unexpectedly blocked"),
    }
}
fn setup() -> Runner<'static, SpiBus<Pin, Spi>> {
    let mut runner = Runner { memory: BTreeMap::new(), writes: 0, fail_write: None, marker: PhantomData };
    runner.put(0x203a34, 0xd035d);
    runner.put(0x203a80, 0x8b634);
    runner.put(0x201f1c, TRANSPORT - 256);
    runner.put(TRANSPORT, 0x44105);
    runner.put(0x200560, 1);
    runner.put(0x200564, ALLOCATION);
    runner.put(0x200568, 0x220000);
    runner.store(0xd035c, &[0x30, 0xb4, 0, 0x24, 0x0c, 0x60, 5, 0x2b, 0x1e, 0xdd, 6, 0x2b, 0x1c, 0xd0, 8, 0x2b]);
    for (i, word) in [0x712f3, 0x7131f, 0x71321, 0x71345, 0x71547, 0x713e3, 0x7143f, 0x714cf, 0, 0x713fd, 0x7154f].into_iter().enumerate() {
        runner.put(0x8b634 + i as u32 * 4, word);
    }
    runner
}

#[test]
fn allocates_once_installs_starts_and_restores_without_reclaiming() {
    let mut runner = setup();
    let mut installed = ready(runtime::install(&mut runner)).unwrap();
    assert_eq!(installed.allocation, ALLOCATION);
    assert_eq!(installed.buffer_size_v2, [0, 251, 0, 4, 30, 0, 4]);
    assert_eq!(runner.word(0x200564), ALLOCATION + 4096);
    assert_eq!(runner.word(TRANSPORT), ALLOCATION + 2049);
    assert_eq!(runner.word(0x203a80), ALLOCATION + 224);
    assert_eq!(runner.word(ALLOCATION + 2056), ALLOCATION);
    assert_eq!(runner.word(ALLOCATION + 2088), ALLOCATION + 3161);
    ready(installed.start(&mut runner)).unwrap();
    ready(installed.restore(&mut runner)).unwrap();
    assert_eq!(runner.word(0x203a34), 0xd035d);
    assert_eq!(runner.word(TRANSPORT), 0x44105);
    assert_eq!(runner.word(0x203a80), 0x8b634);
    assert_eq!(runner.word(0x209788), 0);
    assert_eq!(runner.word(0x200564), ALLOCATION + 4096);
}

#[test]
fn every_ambiguous_write_failure_restores_published_hooks() {
    let mut successful = setup();
    ready(runtime::install(&mut successful)).unwrap();
    for fail in 1..=successful.writes {
        let mut runner = setup();
        runner.fail_write = Some(fail);
        assert!(ready(runtime::install(&mut runner)).is_err(), "write {fail}");
        assert_eq!(runner.word(0x203a34), 0xd035d, "write {fail}");
        assert_eq!(runner.word(TRANSPORT), 0x44105, "write {fail}");
        assert_eq!(runner.word(0x203a80), 0x8b634, "write {fail}");
    }
}

#[test]
fn refuses_busy_radio_or_unknown_stock_pointer_before_writes() {
    for (address, value) in [(0x209788, 1), (TRANSPORT, 0x12345), (0x203a80, 0x12345), (0x203a34, 0x12345)] {
        let mut runner = setup();
        runner.put(address, value);
        assert!(ready(runtime::install(&mut runner)).is_err());
        assert_eq!(runner.writes, 0);
    }
}
