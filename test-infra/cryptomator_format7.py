"""Independent reference for Cryptomator vault format 7 (SIV_CTRMAC file
contents), written from the published specification. Used to check Weave
against code that shares nothing with it except the key wrap helper."""

import base64
import hashlib
import hmac
import json
import os
import sys

from Crypto.Cipher import AES
from pycryptomator.cryptomator import aes_unwrap, aes_wrap

PASSWORD = "format seven password"
CHUNK = 32 * 1024


def ctr(key, nonce, data):
    return AES.new(key, AES.MODE_CTR, nonce=b"", initial_value=nonce).encrypt(data)


def mac(key, data):
    return hmac.new(key, data, hashlib.sha256).digest()


def siv(pk, hk):
    return AES.new(hk + pk, AES.MODE_SIV)


def dir_path(base, pk, hk, dir_id):
    cipher = siv(pk, hk)
    encrypted, tag = cipher.encrypt_and_digest(dir_id.encode())
    digest = base64.b32encode(hashlib.sha1(tag + encrypted).digest()).decode()
    return os.path.join(base, "d", digest[:2], digest[2:])


def encrypt_name(pk, hk, dir_id, name):
    cipher = siv(pk, hk)
    cipher.update(dir_id.encode())
    encrypted, tag = cipher.encrypt_and_digest(name.encode())
    return base64.urlsafe_b64encode(tag + encrypted).decode() + ".c9r"


def decrypt_name(pk, hk, dir_id, name):
    raw = base64.urlsafe_b64decode(name[:-4])
    cipher = siv(pk, hk)
    cipher.update(dir_id.encode())
    return cipher.decrypt_and_verify(raw[16:], raw[:16]).decode()


def encrypt_file(pk, hk, data):
    content_key, header_nonce = os.urandom(32), os.urandom(16)
    payload = ctr(pk, header_nonce, b"\xff" * 8 + content_key)
    out = header_nonce + payload + mac(hk, header_nonce + payload)
    for index in range(0, max(len(data), 1), CHUNK):
        if not data:
            break
        nonce = os.urandom(16)
        body = ctr(content_key, nonce, data[index:index + CHUNK])
        number = (index // CHUNK).to_bytes(8, "big")
        out += nonce + body + mac(hk, header_nonce + number + nonce + body)
    return out


def decrypt_file(pk, hk, blob):
    header_nonce, payload, tag = blob[:16], blob[16:56], blob[56:88]
    assert hmac.compare_digest(tag, mac(hk, header_nonce + payload)), "header MAC"
    content_key = ctr(pk, header_nonce, payload)[8:]
    out, rest, number = b"", blob[88:], 0
    while rest:
        chunk, rest = rest[:CHUNK + 48], rest[CHUNK + 48:]
        nonce, body, tag = chunk[:16], chunk[16:-32], chunk[-32:]
        expected = mac(hk, header_nonce + number.to_bytes(8, "big") + nonce + body)
        assert hmac.compare_digest(tag, expected), f"chunk {number} MAC"
        out += ctr(content_key, nonce, body)
        number += 1
    return out


def keys(base):
    master = json.load(open(os.path.join(base, "masterkey.cryptomator")))
    kek = hashlib.scrypt(PASSWORD.encode(), salt=base64.b64decode(master["scryptSalt"]),
                         n=master["scryptCostParam"], r=master["scryptBlockSize"], p=1,
                         maxmem=0x7fffffff, dklen=32)
    return aes_unwrap(kek, base64.b64decode(master["primaryMasterKey"])), \
        aes_unwrap(kek, base64.b64decode(master["hmacMasterKey"]))


def create(base):
    pk, hk, salt = os.urandom(32), os.urandom(32), os.urandom(8)
    kek = hashlib.scrypt(PASSWORD.encode(), salt=salt, n=16384, r=8, p=1, maxmem=0x7fffffff, dklen=32)
    master = {
        "version": 7,
        "scryptSalt": base64.b64encode(salt).decode(),
        "scryptCostParam": 16384,
        "scryptBlockSize": 8,
        "primaryMasterKey": base64.b64encode(aes_wrap(kek, pk)).decode(),
        "hmacMasterKey": base64.b64encode(aes_wrap(kek, hk)).decode(),
        "versionMac": base64.b64encode(mac(hk, (7).to_bytes(4, "big"))).decode(),
    }
    os.makedirs(base)
    json.dump(master, open(os.path.join(base, "masterkey.cryptomator"), "w"))
    root = dir_path(base, pk, hk, "")
    os.makedirs(root)
    small = encrypt_file(pk, hk, b"written by the format 7 reference")
    open(os.path.join(root, encrypt_name(pk, hk, "", "notes.txt")), "wb").write(small)
    big = bytes(i % 83 for i in range(70000))
    open(os.path.join(root, encrypt_name(pk, hk, "", "big.bin")), "wb").write(encrypt_file(pk, hk, big))


def read(base):
    pk, hk = keys(base)
    root = dir_path(base, pk, hk, "")
    found = {decrypt_name(pk, hk, "", name): name for name in os.listdir(root) if name.endswith(".c9r")}
    blob = open(os.path.join(root, found["from-weave.txt"]), "rb").read()
    assert decrypt_file(pk, hk, blob) == b"written by Weave in format 7", "from-weave.txt"
    blob = open(os.path.join(root, found["weave-big.bin"]), "rb").read()
    assert decrypt_file(pk, hk, blob) == bytes(i % 71 for i in range(70000)), "weave-big.bin"
    print("The reference read Weave's format 7 files.")


if __name__ == "__main__":
    {"create": create, "read": read}[sys.argv[1]](sys.argv[2])
