#!/usr/bin/env python3
"""Build PIC transport callback; --emulate checks real Thumb with mocked ROM I/O."""
import argparse
from collections import deque
import json
from pathlib import Path
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def build(directory, base):
    assembly, code, elf, binary = [directory / name for name in
                                  ('transport-s.o', 'transport-c.o', 'transport.elf', 'transport.bin')]
    subprocess.run(['arm-none-eabi-as', '-mcpu=cortex-m3', '-mthumb',
                    str(ROOT / 'hardware/transport_callback.S'), '-o', str(assembly)], check=True)
    subprocess.run(['arm-none-eabi-gcc', '-std=c11', '-Os', '-mcpu=cortex-m3', '-mthumb',
                    '-ffreestanding', '-fPIC', '-fno-builtin', '-fno-unwind-tables',
                    '-fno-asynchronous-unwind-tables', '-Wall', '-Wextra', '-Werror', '-c',
                    str(ROOT / 'hardware/transport_callback.c'), '-o', str(code)], check=True)
    subprocess.run(['arm-none-eabi-ld', f'-Ttext={base:#x}', '-e', 'transport_callback',
                    str(assembly), str(code), '-o', str(elf)], check=True)
    subprocess.run(['arm-none-eabi-objcopy', '-O', 'binary', '--only-section=.text',
                    str(elf), str(binary)], check=True)
    symbols = {name: int(value, 16) for value, _, name in
               (line.split() for line in subprocess.check_output(['arm-none-eabi-nm', '-n', str(elf)], text=True).splitlines())}
    disassembly = subprocess.check_output(['arm-none-eabi-objdump', '-d', str(elf)], text=True)
    (directory / 'transport.txt').write_text(disassembly)
    blob = binary.read_bytes()
    assert symbols['transport_callback'] == base and symbols['transport_state'] == base + 8
    assert len(blob) <= 1024 and 'dmb\tsy' in disassembly
    assert struct.unpack_from('<10I', blob, 8) == (0, 0x42, 0, 0, 0, 0, 0, 0, 0, 0)
    return blob, symbols


def iso(sequence=0, payload=None, flags=0x2042, length=30, sdu=26):
    payload = bytes(range(26)) if payload is None else payload
    packet = struct.pack('<4H', flags, length, sequence, sdu) + payload
    return len(packet).to_bytes(3, 'little') + b'\x05' + packet + bytes((-4 - len(packet)) % 4)


def check_host_encoding(directory):
    source, executable = directory / 'host-encoding.rs', directory / 'host-encoding'
    source.write_text('''extern crate alloc;
#[path = ISO_SOURCE] mod iso;
use std::io::Write;
fn main() {
    let payload: [u8; 26] = core::array::from_fn(|i| i as u8);
    let packet = iso::IsoPacket { handle: 0x42, boundary: iso::Boundary::Complete,
        sdu: Some(iso::SduHeader { timestamp: None, sequence_number: 0x1234,
            length: 26, status: iso::PacketStatus::Valid }), payload: &payload };
    let mut framed = [0u8; 40];
    let length = packet.encode_into(iso::Direction::HostToController, &mut framed[4..]).unwrap();
    assert_eq!(length, 34);
    framed[0] = length as u8;
    framed[3] = 5;
    assert_eq!(iso::IsoPacket::parse(&framed[4..38], iso::Direction::HostToController).unwrap(), packet);
    std::io::stdout().write_all(&framed).unwrap();
}
'''.replace('ISO_SOURCE', json.dumps(str(ROOT / 'src/iso.rs'))))
    subprocess.run(['rustc', '--edition', '2024', '-A', 'dead_code', str(source), '-o', str(executable)], check=True)
    assert subprocess.check_output([str(executable)]) == iso(0x1234)


def emulate(blob, symbols, base):
    from unicorn import Uc, UC_ARCH_ARM, UC_MODE_THUMB, UC_MODE_MCLASS, UC_HOOK_CODE, UC_HOOK_MEM_WRITE
    from unicorn import arm_const as arm

    class Machine:
        def __init__(self, translated=False):
            self.cpu = Uc(UC_ARCH_ARM, UC_MODE_THUMB | UC_MODE_MCLASS)
            self.cpu.mem_map(0, 0x100000)
            self.cpu.mem_write(0, (ROOT / 'work/cyw43439-btrom-clean.bin').read_bytes())
            self.cpu.mem_map(0x200000, 0x20000)
            self.cpu.mem_write(base, blob)
            self.state = symbols['transport_state']
            self.context, self.ring, self.allocation = 0x207000, 0x204000, 0x208000
            self.meta, self.slots = self.allocation + 1792, self.allocation + 1824
            self.mapping = 0x100000 if translated else 0
            self.put(self.state, self.allocation)
            self.put(0x2038cc + 12, self.ring + self.mapping)
            self.put(0x2038cc + 16, 4096)
            self.put(0x2038cc + 20, 4096)
            self.messages = deque()
            self.next_message = 0x207400
            self.trace, self.writes = [], []
            self.publish_after_empty = None
            self.credit_success = True
            self.credit_during_emit = 0
            self.cpu.hook_add(UC_HOOK_CODE, self.hook)
            self.cpu.hook_add(UC_HOOK_MEM_WRITE, self.write_hook)

        def word(self, address):
            return int.from_bytes(self.cpu.mem_read(address, 4), 'little')

        def put(self, address, value):
            self.cpu.mem_write(address, struct.pack('<I', value))

        def message(self, kind):
            address = self.next_message
            self.next_message += 16
            self.put(address + 4, kind)
            self.messages.append(address)
            return address

        def feed(self, data, offset=None):
            if offset is not None:
                self.put(self.ring + 8192, offset)
                self.put(self.ring + 8196, offset)
            start = self.word(self.ring + 8192)
            for index, byte in enumerate(data):
                self.cpu.mem_write(self.ring + ((start + index) & 4095), bytes([byte]))
            self.put(self.ring + 8192, (start + len(data)) & 4095)

        def write_hook(self, _cpu, _access, address, size, value, _user):
            self.writes.append((address, size, value))
            if address == self.meta:
                old = self.word(self.meta)
                assert value == (old + 1) % 5
                out = self.word(self.ring + 8196)
                expected = bytes(self.cpu.mem_read(self.ring + ((out + 12 + i) & 4095), 1)[0] for i in range(26))
                assert bytes(self.cpu.mem_read(self.slots + old * 40, 26)) == expected

        def hook(self, cpu, address, _size, _user):
            r0 = cpu.reg_read(arm.UC_ARM_REG_R0)
            r1 = cpu.reg_read(arm.UC_ARM_REG_R1)
            value = 0
            if address == 0x21f000:
                cpu.emu_stop()
                return
            if base <= address < base + len(blob):
                return
            if 0x44104 <= address < 0x441b0:
                return  # Execute the real stock callback for equivalence checks.
            if address == self.allocation + 3072 + 88:
                assert 1 <= r0 <= 4
                self.trace.append(('credits', r0, self.credit_success))
                value = int(self.credit_success)
                if self.credit_during_emit:
                    self.put(self.meta + 24, (self.word(self.meta + 24) + self.credit_during_emit) & 0xffffffff)
                    self.credit_during_emit = 0
            elif address == 0x17ed4:
                assert r0 == self.context + 0xd8
                value = self.messages.popleft() if self.messages else 0
            elif address == 0x7baec:
                value = r0 - self.mapping
                assert self.ring <= value <= self.ring + 8196
            elif address == 0x5f940:
                pass
            elif address == 0x5fa0e:
                assert r0 == self.context
                out = self.word(self.ring + 8196)
                raw = bytes(self.cpu.mem_read(self.ring + out, 4))
                assert raw[3] != 5, 'ISO reached stock parser'
                size = int.from_bytes(raw[:3], 'little')
                self.put(self.ring + 8196, (out + size + 7) & ~3 & 4095)
                if self.word(self.ring + 8196) != self.word(self.ring + 8192):
                    self.message(0xc000)
                self.trace.append(('parse', raw[3]))
                value = 0x207800
            elif address == 0x1490e:
                assert r0 == self.context and r1 == 0x207800
                self.trace.append(('dispatch',))
            elif address == 0x44024:
                self.trace.append(('busy', r0))
                if self.publish_after_empty is not None:
                    pending, self.publish_after_empty = self.publish_after_empty, None
                    self.feed(pending)
                    self.message(0xc000)
            elif address == 0x440c2:
                assert r0 == self.context
                value = 1
            elif address == 0x4406a:
                assert r0 == self.context
                self.trace.append(('flush',))
            elif address == 0x440aa:
                assert r0 == self.context
                self.trace.append(('outgoing', self.word(r1 + 4)))
            elif address == 0x17ada:
                self.trace.append(('free', self.word(r0 + 4)))
            elif address == 0x418be:
                assert r0 == 0
                self.trace.append(('hardware_error',))
            elif address in (0x440dc, 0x440e2, 0x5f87a):
                self.trace.append((hex(address),))
            else:
                raise AssertionError(f'unexpected ROM execution {address:#x}')
            cpu.reg_write(arm.UC_ARM_REG_R0, value)
            for register in (arm.UC_ARM_REG_R1, arm.UC_ARM_REG_R2, arm.UC_ARM_REG_R3, arm.UC_ARM_REG_R12):
                cpu.reg_write(register, 0xdeadbeef)
            cpu.reg_write(arm.UC_ARM_REG_PC, cpu.reg_read(arm.UC_ARM_REG_LR))

        def call(self, events=1, entry=None):
            self.cpu.reg_write(arm.UC_ARM_REG_R0, self.context)
            self.cpu.reg_write(arm.UC_ARM_REG_R1, events)
            self.cpu.reg_write(arm.UC_ARM_REG_SP, 0x21ff00)
            self.cpu.reg_write(arm.UC_ARM_REG_LR, 0x21f001)
            saved = [getattr(arm, f'UC_ARM_REG_R{i}') for i in range(4, 12)]
            for i, register in enumerate(saved): self.cpu.reg_write(register, 0x12340000 + i)
            self.cpu.emu_start((base if entry is None else entry) | 1, 0x21f002, count=100000)
            assert self.cpu.reg_read(arm.UC_ARM_REG_PC) == 0x21f000
            assert self.cpu.reg_read(arm.UC_ARM_REG_SP) == 0x21ff00
            assert [self.cpu.reg_read(register) for register in saved] == list(range(0x12340000, 0x12340008))
            for slot in range(5):
                assert bytes(self.cpu.mem_read(self.slots + slot * 40 + 26, 14)) == bytes(14)
            assert bytes(self.cpu.mem_read(self.slots + 200, 24)) == bytes(24)

    for offset in (0, 4, 4060, 4088, 4092):
        machine = Machine(translated=offset == 4088)
        machine.feed(iso(123), offset)
        machine.message(0xc000)
        machine.call()
        assert machine.word(machine.meta) == 1 and machine.word(machine.meta + 8) == 1
        assert machine.word(machine.state + 24) == 123 and machine.word(machine.state + 8) == 0
        assert machine.word(machine.ring + 8196) == (offset + 40) & 4095
        assert not any(event[0] == 'parse' for event in machine.trace)

    command = b'\x03\0\0\x01\x03\x0c\0\0'
    machine = Machine()
    machine.feed(command + iso(1) + command + iso(2))
    machine.message(0xc000)
    machine.message(0xc001)  # Allocation retry and redundant notifications are harmless.
    machine.message(0xc000)
    machine.call()
    assert machine.word(machine.meta + 8) == 2
    assert len([event for event in machine.trace if event[0] == 'parse']) == 2

    machine = Machine()
    machine.message(0xc000)
    machine.publish_after_empty = iso(42)
    machine.call()
    assert machine.word(machine.meta + 8) == 1
    assert not any(event[0] == 'parse' for event in machine.trace)

    machine = Machine()
    machine.feed(b''.join(iso(i, bytes([i]) * 26) for i in range(5)))
    machine.message(0xc000)
    machine.call()
    assert machine.word(machine.meta + 8) == 4 and machine.word(machine.ring + 8196) == 160
    assert machine.word(machine.state + 8) == 1
    first = bytes(machine.cpu.mem_read(machine.slots, 26))
    assert first == bytes(26)
    machine.put(machine.meta + 4, 1)  # Model consumer's copy-before-tail-release.
    # Consumer wakes event bit 0 directly, without allocating a queue message.
    machine.call()
    assert machine.word(machine.meta + 8) == 5 and machine.word(machine.ring + 8196) == 200

    for bad in (iso(flags=0x2043), iso(flags=0x6042), iso(flags=0x0042),
                iso(flags=0xa042), iso(length=29), iso(length=0x401e), iso(sdu=0x401a),
                iso(sdu=0x101a), iso(sdu=25), iso(payload=bytes(25))):
        machine = Machine()
        machine.feed(bad)
        machine.message(0xc000)
        machine.call()
        assert machine.word(machine.state + 8) == 2
        assert machine.word(machine.meta) == 0 and machine.word(machine.ring + 8196) == 0

    for count in (4, 8, 12, 36):
        machine = Machine()
        machine.feed(iso()[:count])
        machine.message(0xc000)
        machine.call()
        assert machine.word(machine.state + 8) == 3 and machine.word(machine.ring + 8196) == 0

    for location, value in ((8192, 4096), (8196, 4096), (8192, 2)):
        machine = Machine()
        machine.feed(iso())
        machine.put(machine.ring + location, value)
        machine.message(0xc000)
        machine.call()
        assert machine.word(machine.state + 8) == 4 and machine.word(machine.meta) == 0

    machine = Machine()
    for kind in (0x7041, 0x7043, 0x7084, 0xc002, 0xc003, 0xc004, 0x1234): machine.message(kind)
    machine.call(events=0)
    assert not machine.trace and machine.word(machine.state + 16) == 0
    machine.call()
    assert machine.trace == [('outgoing', 0x7041), ('outgoing', 0x7043), ('outgoing', 0x7084),
                             ('hardware_error',), ('0x440dc',), ('free', 0xc002),
                             ('0x440dc',), ('free', 0xc003), ('0x440e2',), ('free', 0xc004), ('free', 0x1234)]
    replacement, stock = Machine(), Machine()
    for current in (replacement, stock):
        current.feed(command + command)
        for kind in (0xc001, 0x7041, 0x7043, 0x7084, 0xc002, 0xc003, 0xc004, 0x1234): current.message(kind)
    replacement.call()
    stock.call(entry=0x44104)
    assert replacement.trace == stock.trace, 'stock ROM message behavior changed'
    machine = Machine()
    machine.put(machine.state + 32, machine.allocation + 3072 + 89)
    machine.feed(iso())
    machine.message(0xc000)
    machine.call()
    assert not any(event[0] == 'credits' for event in machine.trace), 'credits before RF completion'
    machine.put(machine.meta + 24, 1)
    machine.credit_success = False
    machine.call()
    assert machine.word(machine.state + 36) == 0, 'lost pending credit on allocation failure'
    machine.credit_success = True
    machine.credit_during_emit = 1
    machine.trace.clear()
    machine.call()
    assert machine.trace == [('credits', 1, True), ('credits', 1, True)]
    assert machine.word(machine.state + 36) == 2, 'completion during send lost or duplicated'
    machine.trace.clear()
    machine.call()
    assert not machine.trace, 'duplicate credit'
    machine.put(machine.state + 36, 0xfffffffe)
    machine.put(machine.meta + 24, 1)
    machine.call()
    assert machine.trace == [('credits', 3, True)] and machine.word(machine.state + 36) == 1
    machine.trace.clear()
    machine.put(machine.meta + 24, 6)
    machine.call()
    assert not machine.trace and machine.word(machine.state + 36) == 1, 'out-of-range credit accepted'
    print(f'Thumb ingress checks pass at {base:#x}: wrap, mapped ring, mixed packets, retries, publication race, backpressure, rejection and message ownership.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--emulate', action='store_true')
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='cyw43-ingress-') as temporary:
        directory = args.output or Path(temporary)
        directory.mkdir(parents=True, exist_ok=True)
        check_host_encoding(directory)
        previous = None
        for base in (0x210000, 0x211000):
            blob, symbols = build(directory, base)
            if previous is not None: assert blob == previous, 'not position independent'
            previous = blob
            if args.emulate: emulate(blob, symbols, base)
        print(f'Transport callback {len(blob)} bytes, position independent at two RAM addresses; state offset 8.')


if __name__ == '__main__':
    main()
