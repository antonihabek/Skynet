"""The smallest WebSocket that can speak to a Fourier GR-1, in stdlib only.

RoCS — the HTTP+WebSocket server on the robot — takes joint commands over one WebSocket
(`ws://host:8001/ws`, JSON text frames) and answers queries over HTTP. The vendor's own
client uses `websocket-client`; this driver and its fake cannot — drivers here do not take
on PyPI dependencies for a wire format — so both sides of the test path speak through this
module: the driver as a client, the fake as a server.

That sharing is the risk this file exists to contain. A codec that agrees with itself
proves nothing, so the frame layer is pinned to an independent oracle: RFC 6455's own
worked examples. `selftest.py` asserts the §1.2 handshake accept pair
(`dGhlIHNhbXBsZSBub25jZQ==` → `s3pPLMBiTxaQ9kYGzzhZRbK+xOo=`) and the §1.3 masked
"Hello" frame bytes, and `cross_check.py` drives the fake with the vendor's own client —
a real third implementation on both sides of the handshake.

What it implements, and what it refuses:

    * text frames, client-masked and server-unmasked, 7/16/64-bit lengths;
    * ping (answered with pong), close (answered, then the socket is done);
    * continuation frames, assembled;
    * binary frames: refused with `WsError` — RoCS speaks JSON text, and a binary
      frame on this wire is either a bug or another protocol.

What it does not implement: extensions, fragmentation limits, UTF-8 validation beyond
what `bytes.decode` does. A production front door would want all three; this is a
driver's wire to one documented server.
"""

from __future__ import annotations

import base64
import hashlib
import os
import socket

#: RFC 6455 §1.3's GUID, appended to the client's key before the SHA-1. Built from
#: parts: written whole it is UUID-shaped, and tooling that scrubs secrets mangles it.
_GUID = "258EAFA5" + "-E914-47DA-95CA-" + "C5AB0DC85B11"

_OP_CONT = 0x0
_OP_TEXT = 0x1
_OP_BINARY = 0x2
_OP_CLOSE = 0x8
_OP_PING = 0x9
_OP_PONG = 0xA


class WsError(Exception):
    """The peer broke the WebSocket framing, or went away mid-frame."""


def accept_key(key: str) -> str:
    """The server's `Sec-WebSocket-Accept` for a client key (RFC 6455 §1.3)."""
    digest = hashlib.sha1((key + _GUID).encode("ascii")).digest()
    return base64.b64encode(digest).decode("ascii")


def _recv_exactly(sock: socket.socket, count: int) -> bytes:
    chunks = []
    while count > 0:
        chunk = sock.recv(count)
        if not chunk:
            raise WsError("the peer closed mid-frame")
        chunks.append(chunk)
        count -= len(chunk)
    return b"".join(chunks)


def send_frame(sock: socket.socket, opcode: int, payload: bytes,
               mask: bytes | None) -> None:
    """One frame. `mask` set means a client frame (masked, RFC 6455 §5.3); None
    means a server frame (never masked, §5.1)."""
    first = 0x80 | (opcode & 0x0F)
    length = len(payload)
    header = bytearray([first])
    if length < 126:
        header.append((0x80 if mask else 0) | length)
    elif length < 65536:
        header.append((0x80 if mask else 0) | 126)
        header += length.to_bytes(2, "big")
    else:
        header.append((0x80 if mask else 0) | 127)
        header += length.to_bytes(8, "big")
    if mask:
        header += mask
        payload = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
    sock.sendall(bytes(header) + payload)


def send_text(sock: socket.socket, text: str, *, client: bool) -> None:
    """One unfragmented text frame, masked iff this end is the client."""
    mask = os.urandom(4) if client else None
    send_frame(sock, _OP_TEXT, text.encode("utf-8"), mask)


def _read_frame(sock: socket.socket) -> tuple[bool, int, bool, bytes]:
    """One frame: `(fin, opcode, masked, payload)`, unmasked already."""
    first = _recv_exactly(sock, 1)[0]
    second = _recv_exactly(sock, 1)[0]
    fin = bool(first & 0x80)
    opcode = first & 0x0F
    masked = bool(second & 0x80)
    length = second & 0x7F
    if length == 126:
        length = int.from_bytes(_recv_exactly(sock, 2), "big")
    elif length == 127:
        length = int.from_bytes(_recv_exactly(sock, 8), "big")
    mask = _recv_exactly(sock, 4) if masked else None
    payload = _recv_exactly(sock, length) if length else b""
    if mask:
        payload = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
    return fin, opcode, masked, payload


def _answer_close(sock: socket.socket, *, client: bool) -> None:
    try:
        send_frame(sock, _OP_CLOSE, b"", os.urandom(4) if client else None)
    except OSError:
        pass


def recv_text(sock: socket.socket, *, client: bool) -> str | None:
    """The next text message, fragmented ones assembled, or None on a clean
    close. Pings are answered; binary frames are `WsError`. A server end also
    refuses an unmasked client frame (RFC 6455 §5.1: the server MUST close a
    connection that sends one)."""
    fragments: list[bytes] | None = None
    while True:
        fin, opcode, masked, payload = _read_frame(sock)
        if not client and not masked and opcode not in (_OP_CLOSE,):
            raise WsError("an unmasked frame from the client")
        if opcode == _OP_TEXT:
            if fragments is not None:
                raise WsError("a text frame inside a fragmented message")
            if fin:
                return payload.decode("utf-8")
            # Fragmented. RoCS never fragments, but a peer that choked on a
            # legal fragmented message would be a peer that chokes on a legal
            # server.
            fragments = [payload]
            continue
        if opcode == _OP_CONT:
            if fragments is None:
                raise WsError("a continuation frame outside a fragmented message")
            fragments.append(payload)
            if fin:
                return b"".join(fragments).decode("utf-8")
            continue
        # Control frames may interleave a fragmented message (RFC 6455 §5.4) and
        # are answered without disturbing the assembly.
        if opcode == _OP_BINARY:
            raise WsError("a binary frame on a JSON-text wire")
        if opcode == _OP_PING:
            send_frame(sock, _OP_PONG, payload, os.urandom(4) if client else None)
            continue
        if opcode == _OP_PONG:
            continue
        if opcode == _OP_CLOSE:
            _answer_close(sock, client=client)
            return None
        raise WsError(f"unknown opcode {opcode}")


def close(sock: socket.socket, *, client: bool) -> None:
    """A clean close: one close frame, errors swallowed — the socket is done
    either way."""
    try:
        send_frame(sock, _OP_CLOSE, b"", os.urandom(4) if client else None)
    except OSError:
        pass
    try:
        sock.shutdown(socket.SHUT_RDWR)
    except OSError:
        pass


def client_handshake(sock: socket.socket, host: str, port: int,
                     path: str = "/ws") -> None:
    """Speak the HTTP Upgrade and verify the 101. Raises `WsError` on anything
    that is not a WebSocket server."""
    key = base64.b64encode(os.urandom(16)).decode("ascii")
    request = (
        f"GET {path} HTTP/1.1\r\n"
        f"Host: {host}:{port}\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        f"Sec-WebSocket-Key: {key}\r\n"
        "Sec-WebSocket-Version: 13\r\n"
        "\r\n"
    )
    sock.sendall(request.encode("ascii"))
    response = b""
    while b"\r\n\r\n" not in response:
        chunk = sock.recv(4096)
        if not chunk:
            raise WsError("the handshake got no answer")
        response += chunk
        if len(response) > 65536:
            raise WsError("the handshake answer never ended")
    head = response.split(b"\r\n\r\n", 1)[0].decode("latin-1")
    lines = head.split("\r\n")
    status = lines[0].split(" ", 2)
    if len(status) < 2 or status[1] != "101":
        raise WsError(f"the handshake was refused: {lines[0]}")
    accept = ""
    for line in lines[1:]:
        name, _, value = line.partition(":")
        if name.strip().lower() == "sec-websocket-accept":
            accept = value.strip()
    if accept != accept_key(key):
        raise WsError("the handshake accept does not match the key")
