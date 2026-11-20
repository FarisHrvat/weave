"""Cross-checks Weave's Cryptomator support against pycryptomator.

Usage: cryptomator_interop.py write|read <work dir>
  write: create a vault with pycryptomator for Weave to read
  read:  open the vault Weave wrote and check its contents
"""
import os
import sys

from pycryptomator.cryptomator import Vault, init_vault

PASSWORD = "interop password"
step, work = sys.argv[1], sys.argv[2]

if step == "write":
    vault_dir = os.path.join(work, "python-vault")
    os.makedirs(vault_dir)
    init_vault(vault_dir, PASSWORD)
    vault = Vault(vault_dir, PASSWORD)
    source = os.path.join(work, "python-source")
    os.makedirs(source)
    with open(os.path.join(source, "readme.txt"), "w") as handle:
        handle.write("hello from Python")
    with open(os.path.join(source, "data.bin"), "wb") as handle:
        handle.write(bytes(i % 89 for i in range(70_000)))
    vault.encryptFile(os.path.join(source, "readme.txt"), "/readme.txt")
    vault.mkdir("/Docs")
    vault.encryptFile(os.path.join(source, "data.bin"), "/Docs/data.bin")
elif step == "read":
    vault = Vault(os.path.join(work, "weave-vault"), PASSWORD)
    out = os.path.join(work, "python-out")
    os.makedirs(out)
    vault.decryptFile("/Folder/hello.txt", os.path.join(out, "hello.txt"))
    vault.decryptFile("/Folder/big.bin", os.path.join(out, "big.bin"))
    long_name = "n" * 240 + ".txt"
    vault.decryptFile("/" + long_name, os.path.join(out, "long.txt"))
    assert open(os.path.join(out, "hello.txt")).read() == "hello from Weave"
    assert open(os.path.join(out, "big.bin"), "rb").read() == bytes(i % 97 for i in range(70_000))
    assert open(os.path.join(out, "long.txt")).read() == "long"
    print("pycryptomator read the Weave vault correctly")
else:
    sys.exit("unknown step")
