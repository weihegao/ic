#!/usr/bin/env python3
"""Extract icp-cli's embedded proxy canister wasm from the `icp` binary.

icp-cli installs this proxy into the local networks it manages; on a self-hosted
testnet we install it ourselves and point `icp deploy --proxy` at it.

    extract_proxy_wasm.py [path/to/icp] [out.wasm]
"""
import shutil
import sys

MAGIC = b"\x00asm\x01\x00\x00\x00"


def leb128(b, i):
    result = shift = 0
    while True:
        x = b[i]
        i += 1
        result |= (x & 0x7F) << shift
        shift += 7
        if x < 0x80:
            return result, i


def module_end(b, start):
    """Walk the sections of the module at `start`; stop at the next module or at junk."""
    i = start + len(MAGIC)
    while i < len(b) and b[i : i + 8] != MAGIC and b[i] <= 12:
        size, j = leb128(b, i + 1)
        if j + size > len(b):
            break
        i = j + size
    return i


icp = sys.argv[1] if len(sys.argv) > 1 else shutil.which("icp")
out = sys.argv[2] if len(sys.argv) > 2 else "proxy.wasm"
data = open(icp, "rb").read()
start = data.find(MAGIC)
while start != -1:
    module = data[start : module_end(data, start)]
    if b"proxy : (ProxyArgs) -> (ProxyResult)" in module:
        open(out, "wb").write(module)
        print(f"wrote {out} ({len(module)} bytes) from {icp}")
        sys.exit(0)
    start = data.find(MAGIC, start + 1)
sys.exit(f"no proxy canister wasm found in {icp}")
