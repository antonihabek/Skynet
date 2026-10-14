"""The TLS fixtures of the Spot substrate, made when they are first needed.

The fake robot (`fake_spot`) serves gRPC over TLS, and the driver and the vendor's own client are
pointed at a test trust root. That trust root used to be committed, key included. A private key in
a public repository is a thing scanners rightly report, even when it is a throwaway, so the
fixtures are made here instead, with the `openssl` command-line tool, into `drivers/spot/certs/`
(untracked): a certificate authority, whose key is deleted as soon as it has signed, and a server
certificate for `localhost` and every authority the fake lists.

Nothing here disables verification. The files land where the suites, the cross-check and
`run-check.sh` already look for them, so none of them changed.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile

CERTS_DIR = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "certs")

_FILES = ("ca.pem", "server.pem", "server-key.pem")
_DAYS = "3650"


def _run(*args: str) -> None:
    try:
        subprocess.run(args, check=True, capture_output=True, text=True)
    except FileNotFoundError as error:
        raise RuntimeError("the Spot substrate makes its TLS test certificates with the `openssl` "
                           "command, which is not installed") from error
    except subprocess.CalledProcessError as error:
        raise RuntimeError(f"`{' '.join(args[:3])} ...` failed: {error.stderr.strip()}") from error


def _usable(directory: str) -> bool:
    """All three files are there and the server certificate has more than a day left."""
    if not all(os.path.isfile(os.path.join(directory, name)) for name in _FILES):
        return False
    return subprocess.run(("openssl", "x509", "-checkend", "86400", "-noout", "-in",
                           os.path.join(directory, "server.pem")),
                          capture_output=True).returncode == 0


def ensure(authorities: list[str]) -> str:
    """Make the fixtures if they are not there (or are about to expire) and return their
    directory. Safe to call from several processes at once: the set is made whole in a
    directory of its own and moved into place in one step, and the loser of that race uses
    the winner's."""
    if _usable(CERTS_DIR):
        return CERTS_DIR
    parent = os.path.dirname(CERTS_DIR)
    work = tempfile.mkdtemp(prefix=".certs-", dir=parent)
    try:
        ca_key = os.path.join(work, "ca-key.pem")
        ca = os.path.join(work, "ca.pem")
        key = os.path.join(work, "server-key.pem")
        csr = os.path.join(work, "server.csr")
        extension = os.path.join(work, "server.ext")
        _run("openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", ca_key,
             "-out", ca, "-days", _DAYS, "-subj", "/CN=skynet-spot-fake-ca")
        _run("openssl", "req", "-newkey", "rsa:2048", "-nodes", "-keyout", key, "-out", csr,
             "-subj", "/CN=localhost")
        names = ",".join([f"DNS:{name}" for name in authorities] + ["DNS:localhost", "IP:127.0.0.1"])
        with open(extension, "w", encoding="utf-8") as handle:
            handle.write(f"subjectAltName={names}\nbasicConstraints=CA:FALSE\n")
        _run("openssl", "x509", "-req", "-in", csr, "-CA", ca, "-CAkey", ca_key,
             "-CAcreateserial", "-out", os.path.join(work, "server.pem"), "-days", _DAYS,
             "-extfile", extension)
        for leftover in ("ca-key.pem", "server.csr", "server.ext", "ca.srl"):
            os.remove(os.path.join(work, leftover))
        if os.path.isdir(CERTS_DIR) and not _usable(CERTS_DIR):
            shutil.rmtree(CERTS_DIR, ignore_errors=True)
        try:
            os.rename(work, CERTS_DIR)
        except OSError:
            if not _usable(CERTS_DIR):
                raise
    finally:
        shutil.rmtree(work, ignore_errors=True)
    return CERTS_DIR
