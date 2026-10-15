#!/usr/bin/env python3
"""Compile actual raw-command helpers against a CYW43 prefix/padding-write mock."""
from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
source = (ROOT / "hardware/picow_runtime_probe.c").read_text()
functions = []
for name in ("tx_command", "state7_command"):
    match = re.search(r"^static bool " + name + r"\([^\n]*\) \{.*?^\}", source, re.M | re.S)
    assert match, name
    functions.append(match[0])
probe = (ROOT / "hardware/picow_hci_probe.c").read_text()
raw = re.search(r"^static void send_raw_iso_command\(void\) \{.*?^\}", probe, re.M | re.S)
opcodes = re.search(r"^static const uint16_t raw_iso_opcodes\[\] = \{.*?^\};", probe, re.M | re.S)
assert raw and opcodes

harness = r'''
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#define printf(...) ((void)0)
#define TIMEOUT_MS 10
#define PROBE_TAG "CHECK"
typedef unsigned absolute_time_t;
static bool tx_command_done, command_done, command_success;
static uint8_t tx_command_status, command_status;
static uint16_t tx_opcode, state7_opcode;
static unsigned sends;
static int send_error;
static bool available = true;
static const uint8_t *expected;
static size_t expected_length;
static absolute_time_t make_timeout_time_ms(unsigned ms) { return ms; }
static bool time_reached(absolute_time_t deadline) { (void)deadline; return !available; }
static bool hci_can_send_command_packet_now(void) { return available; }
static void poll_once(void) { assert(false); }
static uint16_t little_endian_read_16(const uint8_t *p, unsigned offset) {
    return p[offset] | ((uint16_t)p[offset + 1] << 8);
}
static int hci_send_cmd_packet(uint8_t *packet, int size) {
    ++sends;
    assert(size >= 3 && size <= 258 && (size_t)size == expected_length);
    assert(((uintptr_t)packet & 3) == 0);
    assert(packet != expected && memcmp(packet, expected, size) == 0);
    /* Exact SDK transport contract: four bytes before payload, padded word read. */
    uint8_t *wire = packet - 4;
    wire[0] = size; wire[1] = size >> 8; wire[2] = size >> 16; wire[3] = 1;
    size_t rounded = ((size_t)size + 7) & ~(size_t)3;
    for (size_t i = (size_t)size + 4; i < rounded; ++i) assert(wire[i] == 0);
    volatile uint32_t checksum = 0;
    for (size_t i = 0; i < rounded; i += 4) {
        uint32_t value;
        memcpy(&value, wire + i, 4);
        checksum ^= value;
    }
    (void)checksum;
    tx_command_done = command_done = command_success = true;
    tx_command_status = command_status = 0;
    return send_error;
}
'''
harness += "\n".join(functions)
harness += "\nstatic uint16_t raw_opcode;\nstatic unsigned raw_index;\n" + opcodes[0] + "\n" + raw[0]
harness += r'''
int main(void) {
    bool (*calls[])(uint8_t *, size_t) = {tx_command, state7_command};
    for (size_t call = 0; call < 2; ++call) {
        for (unsigned parameters = 0; parameters <= 255; ++parameters) {
            uint8_t storage[300], original[300];
            memset(storage, 0xa5, sizeof(storage));
            /* Caller may be unaligned and must not expose transport headroom. */
            uint8_t *command = storage + 17;
            command[0] = 0x60; command[1] = 0x20; command[2] = parameters;
            for (unsigned i = 0; i < parameters; ++i) command[3 + i] = i;
            memcpy(original, storage, sizeof(storage));
            expected = command; expected_length = parameters + 3;
            unsigned before = sends;
            assert(calls[call](command, expected_length));
            assert(sends == before + 1 && memcmp(storage, original, sizeof(storage)) == 0);
            assert((call == 0 ? tx_opcode : state7_opcode) == 0x2060);
            send_error = 1;
            assert(!calls[call](command, expected_length));
            assert(memcmp(storage, original, sizeof(storage)) == 0);
            send_error = 0;
        }
        uint8_t invalid[3] = {0x60, 0x20, 0};
        unsigned before = sends;
        for (size_t size = 0; size < 3; ++size) assert(!calls[call](invalid, size));
        assert(!calls[call](invalid, 4));
        assert(!calls[call](invalid, 259));
        assert(!calls[call](invalid, SIZE_MAX));
        assert(sends == before);
        available = false;
        assert(!calls[call](invalid, 3));
        available = true;
        assert(sends == before);
    }
    for (size_t i = 0; i < sizeof(raw_iso_opcodes) / sizeof(raw_iso_opcodes[0]); ++i) {
        uint8_t command[] = {raw_iso_opcodes[i], raw_iso_opcodes[i] >> 8, 0};
        expected = command; expected_length = 3;
        unsigned before = sends;
        send_raw_iso_command();
        assert(sends == before + 1 && raw_index == i + 1 && raw_opcode == raw_iso_opcodes[i]);
    }
    puts("Command headroom: three actual helpers, all parameter lengths/opcodes, prefix/padding, canaries and errors pass");
}
'''
with tempfile.TemporaryDirectory(prefix="cyw43-headroom-") as directory:
    path = Path(directory)
    (path / "check.c").write_text(harness)
    subprocess.run(["cc", "-std=c11", "-O1", "-g", "-Wall", "-Wextra", "-Werror",
                    "-fsanitize=address,undefined", "-fno-omit-frame-pointer",
                    str(path / "check.c"), "-o", str(path / "check")], check=True)
    subprocess.run([str(path / "check")], check=True)
