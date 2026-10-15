#!/usr/bin/env python3
"""Check retained CYW43439 config evidence; requires OpenSSL and cryptography."""

import ctypes
import ctypes.util
import hashlib
from pathlib import Path
import re

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.asymmetric import ec, utils


ROOT = Path(__file__).resolve().parents[1]


class SHA256Context(ctypes.Structure):
    # OpenSSL's public SHA256_CTX layout, declared in openssl/sha.h.
    _fields_ = [("h", ctypes.c_uint32 * 8), ("bits", ctypes.c_uint32 * 2),
                ("data", ctypes.c_uint32 * 16), ("num", ctypes.c_uint),
                ("md_len", ctypes.c_uint)]


crypto = ctypes.CDLL(ctypes.util.find_library("crypto"))
crypto.SHA256_Init.argtypes = [ctypes.POINTER(SHA256Context)]
crypto.SHA256_Init.restype = ctypes.c_int
crypto.SHA256_Update.argtypes = [ctypes.POINTER(SHA256Context),
                               ctypes.c_void_p, ctypes.c_size_t]
crypto.SHA256_Update.restype = ctypes.c_int


def compression_state(data):
    assert len(data) % 64 == 0
    context = SHA256Context()
    assert crypto.SHA256_Init(ctypes.byref(context)) == 1
    assert crypto.SHA256_Update(ctypes.byref(context), data, len(data)) == 1
    assert context.num == 0
    return b"".join(word.to_bytes(4, "big") for word in context.h)


def config_digest(config):
    assert config[:8] == b"BRCMcfgD"
    length = 16 + int.from_bytes(config[12:16], "little")
    assert length + 64 == len(config)
    # ponytail: block-aligned configs only; model ROM final padding for other lengths.
    # ROM 0x3eb20 skips padding when buffered length is zero.
    return compression_state(config[:length][::-1])[::-1]


def extract_config(path):
    data = (ROOT / path).read_bytes()
    offset = data[0] + 2
    base = 0
    memory = {}
    while offset < len(data):
        assert offset + 4 <= len(data)
        length, high, low, kind = data[offset:offset + 4]
        offset += 4
        payload = data[offset:offset + length]
        assert len(payload) == length
        offset += length
        if kind == 1:
            assert length == 0 and offset == len(data)
            break
        if kind == 4:
            assert length == 2
            base = int.from_bytes(payload, "big") << 16
        else:
            assert kind == 0 and length > 0
            address = base + (high << 8) + low
            memory.update((address + i, byte) for i, byte in enumerate(payload))
    return bytes(memory[address] for address in range(0x21f000, 0x21fc80))


def captures(name):
    text = (ROOT / "work/config-validation" / name).read_text()
    passes = re.findall(r"CONFIG_BEGIN startup=(\d+)\n(.*?)CONFIG_END", text, re.S)
    assert len(passes) >= 2, "Need two complete diagnostic captures"
    result = []
    for startup, body in passes:
        memory = {}
        for address, hex_bytes in re.findall(r"^@([0-9a-f]+):([0-9a-f]+)$", body, re.M):
            memory.update((int(address, 16) + i, byte)
                          for i, byte in enumerate(bytes.fromhex(hex_bytes)))
        result.append((int(startup), memory))
    return result


def read(memory, address, length):
    return bytes(memory[address + i] for i in range(length))


def verify(key, config):
    signature = config[-64:]
    encoded = utils.encode_dss_signature(int.from_bytes(signature[:32], "big"),
                                         int.from_bytes(signature[32:], "big"))
    try:
        key.verify(encoded, config_digest(config),
                   ec.ECDSA(utils.Prehashed(hashes.SHA256())))
    except InvalidSignature:
        return False
    return True


def main():
    padded = b"abc\x80" + bytes(52) + (24).to_bytes(8, "big")
    assert compression_state(padded) == hashlib.sha256(b"abc").digest()
    stock = extract_config("work/sdk0065.bin")
    name = extract_config("work/hook-controls/config-string.bin")
    assert [(i, a, b) for i, (a, b) in enumerate(zip(stock, name)) if a != b] == [
        (0x29, ord("I"), ord("J"))]
    stock_passes = captures("stock.txt")
    name_passes = captures("config-name.txt")
    key_bytes = read(stock_passes[0][1], 0x192004ee, 66)
    assert sum(8 - byte.bit_count() for byte in key_bytes[:64]) == 248
    assert int.from_bytes(key_bytes[64:], "little") == 248
    key = ec.EllipticCurvePublicNumbers(
        int.from_bytes(key_bytes[:32], "little"),
        int.from_bytes(key_bytes[32:64], "little"), ec.SECP256R1()).public_key()
    for expected, expected_startup, passes in [
        (stock, 0, stock_passes), (name, 180, name_passes)
    ]:
        for startup, memory in passes:
            assert startup == expected_startup
            assert read(memory, 0x1921f000, 0xc80) == expected
            assert read(memory, 0x192004ee, 66) == key_bytes
            assert int.from_bytes(read(memory, 0x19200688, 4), "little") == 0x2004ee
            ready = bool(int.from_bytes(read(memory, 0x18000c7c, 4), "little") & (1 << 24))
            assert ready == (startup == 0)
            assert read(memory, 0x19200681, 1) == b"\x01"
            assert read(memory, 0x19200682, 1) == bytes([startup == 0])
            assert read(memory, 0x19200684, 1) == b"\x01"
            if startup != 0:
                assert read(memory, 0x192006f4, 32) == config_digest(expected)
    # Scratch survives the first failed pass, but is reused before the second.
    assert read(name_passes[0][1], 0x19209bac, 64) == name[-64:]
    assert read(name_passes[0][1], 0x19209bec, 64) == key_bytes[:64]
    for label, path, expected in [
        ("stock", "work/sdk0065.bin", True),
        ("append-only", "work/hook-controls/append-only.bin", True),
        ("redirect", "work/hook-controls/redirect.bin", False),
        ("config-string", "work/hook-controls/config-string.bin", False),
    ]:
        config = extract_config(path)
        valid = verify(key, config)
        assert valid == expected, label
        print(f"{label}: signature={'valid' if valid else 'invalid'} digest={config_digest(config).hex()}")
    print("PASS: two-pass readback, key integrity, ready status, live failed digest")


if __name__ == "__main__":
    main()
