"""Write the E2E desk fixtures: a two-employee roster and an auditor UFVK.

Both are correctly encoded ZIP-316 testnet strings (F4Jumble + Bech32m, HRP `utest` /
`uviewtest`) built from fixed synthetic bytes, the same way the Rust unit tests build
`employee::tests::ua('p')`. They belong to no wallet and are never used on chain: the
desk only displays them. Run: python3 -I e2e/make-fixtures.py
"""
import hashlib
import os

CHARSET = "qpzry9x8gf2tvdw0s3jn54khce6mua7l"
BECH32M = 0x2BC830A3


def polymod(values):
    gen = [0x3B6A57B2, 0x26508E6D, 0x1EA119FA, 0x3D4233DD, 0x2A1462B3]
    chk = 1
    for v in values:
        b = chk >> 25
        chk = (chk & 0x1FFFFFF) << 5 ^ v
        for i in range(5):
            chk ^= gen[i] if (b >> i) & 1 else 0
    return chk


def bech32m(hrp, data8):
    acc, bits, data = 0, 0, []
    for b in data8:
        acc = (acc << 8) | b
        bits += 8
        while bits >= 5:
            bits -= 5
            data.append((acc >> bits) & 31)
    if bits:
        data.append((acc << (5 - bits)) & 31)
    exp = [ord(c) >> 5 for c in hrp] + [0] + [ord(c) & 31 for c in hrp]
    pm = polymod(exp + data + [0] * 6) ^ BECH32M
    chk = [(pm >> 5 * (5 - i)) & 31 for i in range(6)]
    return hrp + "1" + "".join(CHARSET[d] for d in data + chk)


def xor(a, b):
    return bytes(x ^ y for x, y in zip(a, b))


def f4jumble(m):
    ll = min(64, len(m) // 2)
    lr = len(m) - ll
    H = lambda i, u: hashlib.blake2b(u, digest_size=ll, person=b"UA_F4Jumble_H" + bytes([i, 0, 0])).digest()
    G = lambda i, u: b"".join(
        hashlib.blake2b(u, digest_size=64, person=b"UA_F4Jumble_G" + bytes([i]) + j.to_bytes(2, "little")).digest()
        for j in range((lr + 63) // 64)
    )[:lr]
    a, b = m[:ll], m[ll:]
    x = xor(b, G(0, a))
    y = xor(a, H(0, x))
    d = xor(x, G(1, y))
    c = xor(y, H(1, d))
    return c + d


def unified(hrp, typecode, payload):
    raw = bytes([typecode, len(payload)]) + payload + hrp.encode().ljust(16, b"\0")
    return bech32m(hrp, f4jumble(raw))


here = os.path.dirname(os.path.abspath(__file__))
out = os.path.join(here, "fixtures")
os.makedirs(out, exist_ok=True)
# Orchard receiver (typecode 3, 43 bytes), like ua('p') / ua('q') in src/employee.rs tests.
alice = unified("utest", 0x03, bytes([ord("p")]) * 43)
bob = unified("utest", 0x03, bytes([ord("q")]) * 43)
with open(os.path.join(out, "roster.csv"), "w") as f:
    f.write("name,ua,amountZEC,memo\n")
    f.write(f'Alice,{alice},0.02,"Oct 2026 demo pay"\n')
    f.write(f'Bob,{bob},0.02,"Oct 2026 demo pay"\n')
with open(os.path.join(out, "empty-roster.csv"), "w") as f:
    f.write("name,ua,amountZEC,memo\n")
# Orchard full viewing key item (typecode 3, 96 bytes): fixed synthetic bytes, view-only by type.
with open(os.path.join(out, "auditor-ufvk.txt"), "w") as f:
    f.write(unified("uviewtest", 0x03, bytes(range(96))) + "\n")
print("wrote", out)
