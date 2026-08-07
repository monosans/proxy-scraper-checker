#!/usr/bin/env python3
"""Local HTTP CONNECT proxy that terminates TLS, for the `tls` bench workload.

The `check` workload points every proxy at a refused loopback port, so
`Proxy::check` dies inside connect() and no TLS ever happens. That leaves
aws-lc-sys, by far the largest C allocator linked into this program, idle, and
aws-lc-sys is the allocation traffic that the mimalloc `override` and jemalloc
`override_allocator_on_supported_platforms` features redirect. A benchmark that
never handshakes cannot say anything about them, and it misses the dominant
per-proxy CPU cost of a real run too.

This process is both halves of the target: it speaks HTTP CONNECT, answers 200,
then performs a TLS server handshake on the tunnelled socket. The client is the
real program under test.

src/http.rs:38-44 replaces kx_groups for TlsProfile::Checking with
X25519/P-256/P-384 (post-quantum applies only to scraping) and disables
resumption, so every check is a full classical handshake. Stock OpenSSL
negotiates that exactly, so this helper needs no Rust and no aws-lc-rs of its
own.

The certificate is self-signed, so rustls-platform-verifier rejects it and the
check fails. That is deliberate and deterministic. The handshake still runs key
generation, the ECDHE agreement, the HKDF key schedule, AEAD decryption of the
handshake records and X.509 DER parsing, which is the bulk of per-handshake
allocation. It never reaches the CertificateVerify signature check or the
application-data phase. Trusting the cert instead would mean installing a CA
into three different OS trust stores, which is not worth the fragility.

Usage: python3 bench/tls_server.py --port 8443 --cert cert.pem --key key.pem
Prints a line "READY <port>" once listening, so callers can wait on it rather
than sleeping.
"""

from __future__ import annotations

import argparse
import errno
import socket
import ssl
import struct
import time
import sys
import threading
from concurrent.futures import ThreadPoolExecutor

# Enough to absorb max_concurrent_checks=512 arriving at once; the handshake
# itself releases the GIL inside OpenSSL, so a modest pool keeps up.
WORKERS = 64
BACKLOG = 2048
HEADER_LIMIT = 64 * 1024
IO_TIMEOUT = 15.0
# Every 10, not every 100: the file is the only evidence the tunnel carried
# TLS, and at 100 a short smoke run finishes without ever writing, which looks
# like zero handshakes, the exact failure it is meant to detect. 1200 tiny
# writes over a full workload cost nothing, and they happen in this process
# rather than in the one being measured.
FLUSH_EVERY = 10

# Running out of descriptors, buffers or a client hanging up mid-accept are all
# recoverable; only a closed listener is not.
TRANSIENT_ACCEPT_ERRORS = frozenset(
    getattr(errno, name)
    for name in (
        "EMFILE",
        "ENFILE",
        "ECONNABORTED",
        "EINTR",
        "EAGAIN",
        "EWOULDBLOCK",
        "ENOBUFS",
        "ENOMEM",
    )
    if hasattr(errno, name)
)

CONTEXT: ssl.SSLContext
COUNT_FILE: str | None = None
_handshakes = 0
_lock = threading.Lock()


def record_handshake() -> None:
    """Count completed handshakes so the caller can prove they happened.

    Without this a broken tunnel is invisible: "Started checking 2000 proxies"
    is logged before the first check runs, so the harness's proxies_checked
    guard passes whether the 2000 checks did a TLS handshake or died in
    connect(). The count is written from this process, never from the process
    being measured, so it cannot perturb the measurement. It is flushed every
    FLUSH_EVERY handshakes; the caller only needs the order of magnitude.
    """
    global _handshakes
    # The write stays inside the lock: released first, a thread that read an
    # older total can win the race to the file and make the count go backwards.
    with _lock:
        _handshakes += 1
        if _handshakes % FLUSH_EVERY or not COUNT_FILE:
            return
        try:
            with open(COUNT_FILE, "w", encoding="utf-8") as fh:
                fh.write(f"{_handshakes}\n")
        except OSError:
            pass


def handle(conn: socket.socket) -> None:
    try:
        conn.settimeout(IO_TIMEOUT)
        conn.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

        # Close with an RST rather than a FIN, and set it HERE, before
        # wrap_socket. A graceful close parks the 4-tuple in TIME_WAIT on
        # whichever side closed, usually the client, and macOS offers ~16384
        # ephemeral ports with no loopback TIME_WAIT reuse, so a job simply
        # stops being able to connect partway through. Two macos-26 jobs died
        # at 16400 and 16470 handshakes, which is that limit almost exactly.
        #
        # It has to precede wrap_socket because wrap_socket DETACHES this
        # socket: conn.fileno() becomes -1 and every later setsockopt on it
        # fails. Setting it down in the finally block instead does nothing at
        # all, silently, and the port exhaustion comes back unchanged. The
        # option lives on the descriptor, so it survives the detach and applies
        # when the SSLSocket eventually closes.
        try:
            conn.setsockopt(
                socket.SOL_SOCKET,
                socket.SO_LINGER,
                struct.pack("ii", 1, 0),
            )
        except OSError:
            pass

        # Read the CONNECT request. The target host:port is ignored, since this
        # process is the target, but it must be consumed before the tunnel
        # turns into TLS bytes.
        buf = b""
        while b"\r\n\r\n" not in buf:
            chunk = conn.recv(4096)
            if not chunk:
                return
            buf += chunk
            if len(buf) > HEADER_LIMIT:
                return
        if not buf.upper().startswith(b"CONNECT"):
            conn.sendall(b"HTTP/1.1 405 Method Not Allowed\r\n\r\n")
            return

        conn.sendall(b"HTTP/1.1 200 Connection established\r\n\r\n")

        # wrap_socket performs the server handshake. The client rejects the
        # self-signed certificate and answers with a fatal alert, by which
        # point both sides have done the work this workload measures.
        #
        # Counted before the attempt rather than after it: that fatal alert
        # frequently comes with a reset, which surfaces here as
        # ConnectionResetError rather than SSLError, and counting only the
        # clean endings undercounted by nearly half. All this number has to
        # show is that the tunnel carried TLS at all.
        record_handshake()
        try:
            tls = CONTEXT.wrap_socket(conn, server_side=True)
        except (ssl.SSLError, OSError):
            return
        try:
            tls.close()
        except OSError:
            pass
    except OSError:
        pass
    finally:
        # No-op once wrap_socket has detached it; still needed for the paths
        # that return before the handshake.
        try:
            conn.close()
        except OSError:
            pass


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--cert", required=True)
    parser.add_argument("--key", required=True)
    parser.add_argument("--count-file")
    args = parser.parse_args()

    global CONTEXT, COUNT_FILE
    COUNT_FILE = args.count_file
    if COUNT_FILE:
        with open(COUNT_FILE, "w", encoding="utf-8") as fh:
            fh.write("0\n")
    CONTEXT = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    CONTEXT.load_cert_chain(args.cert, args.key)
    # The client offers only http/1.1 for checking (src/http.rs:70).
    try:
        CONTEXT.set_alpn_protocols(["http/1.1"])
    except NotImplementedError:
        pass

    # max_concurrent_checks is 512, so that many connections can be in flight
    # at once and macOS ships a 256 soft limit. The checked program raises its
    # own limit in src/config.rs; this one has to raise its own.
    try:
        import resource  # noqa: PLC0415  - Unix only, absent on Windows

        soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
        want = min(hard, max(soft, 8192))
        if want > soft:
            resource.setrlimit(resource.RLIMIT_NOFILE, (want, hard))
    except (ImportError, ValueError, OSError):
        pass

    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    # Not SO_REUSEADDR on Windows: there it means what SO_REUSEPORT means
    # elsewhere, so a second instance binds the same port successfully and the
    # kernel splits incoming connections between them. That silently divides
    # the handshake count, seen locally with four listeners on one port.
    # SO_EXCLUSIVEADDRUSE makes the second bind fail loudly instead.
    if hasattr(socket, "SO_EXCLUSIVEADDRUSE"):
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
    else:
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", args.port))
    listener.listen(BACKLOG)

    print(f"READY {args.port}", flush=True)

    with ThreadPoolExecutor(max_workers=WORKERS) as pool:
        try:
            while True:
                try:
                    conn, _ = listener.accept()
                except OSError as exc:
                    # A transient accept() failure must not end the server.
                    # Breaking out of the loop on any OSError closes the
                    # listener and makes every remaining connection refuse
                    # instantly, which in the results is indistinguishable
                    # from an allocator that suddenly needed a third of the
                    # memory.
                    if exc.errno in TRANSIENT_ACCEPT_ERRORS:
                        time.sleep(0.05)
                        continue
                    break
                pool.submit(handle, conn)
        except KeyboardInterrupt:
            pass
        finally:
            listener.close()


if __name__ == "__main__":
    sys.exit(main())
