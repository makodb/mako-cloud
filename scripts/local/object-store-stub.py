#!/usr/bin/env python3
"""A minimal S3-compatible object store for local Mako development.

Mako's object store client (crates/mako-object-store/src/s3.rs) uses exactly
five calls, all path-style over plain HTTP on loopback:
  PUT  /bucket              create the bucket (200, or 409 if present)
  HEAD /bucket              the bucket exists (200) or not (404)
  PUT  /bucket/key          If-None-Match: * -> 200, or 412 if the key exists
  GET  /bucket/key          200 with the bytes, or 404
  DELETE /bucket/key        204
Every request is SigV4-signed; the signature is checked against the access and
secret key files the services use, so a wrong key is refused as S3 would.
Objects are files under ROOT. Not for production: no listing, no multipart.

Usage: object-store-stub.py <port> <root> <access-key-file> <secret-key-file>
(scripts/local/dev-services.sh starts it with the values in .env.)
"""
import hashlib
import hmac
import os
import sys
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT, ROOT = int(sys.argv[1]), os.path.abspath(sys.argv[2])
ACCESS_KEY = open(sys.argv[3]).read().strip()
SECRET_KEY = open(sys.argv[4]).read().strip()


def _hmac(key: bytes, message: str) -> bytes:
    return hmac.new(key, message.encode(), hashlib.sha256).digest()


def signature_valid(method, raw_path, headers, body) -> bool:
    authorization = headers.get("authorization", "")
    if not authorization.startswith("AWS4-HMAC-SHA256 "):
        return False
    fields = dict(
        part.strip().split("=", 1) for part in authorization[len("AWS4-HMAC-SHA256 "):].split(",")
    )
    try:
        access_key, date, region, service, terminal = fields["Credential"].split("/")
        signed = fields["SignedHeaders"].split(";")
        signature = fields["Signature"]
    except (KeyError, ValueError):
        return False
    payload_hash = headers.get("x-amz-content-sha256", "")
    if access_key != ACCESS_KEY or service != "s3" or terminal != "aws4_request":
        return False
    if payload_hash != hashlib.sha256(body).hexdigest():
        return False
    path, _, query = raw_path.partition("?")
    canonical = "\n".join([
        method,
        path,
        query,
        "".join(f"{name}:{headers.get(name, '').strip()}\n" for name in signed),
        ";".join(signed),
        payload_hash,
    ])
    to_sign = "\n".join([
        "AWS4-HMAC-SHA256",
        headers.get("x-amz-date", ""),
        f"{date}/{region}/{service}/{terminal}",
        hashlib.sha256(canonical.encode()).hexdigest(),
    ])
    key = _hmac(_hmac(_hmac(_hmac(f"AWS4{SECRET_KEY}".encode(), date), region), service), terminal)
    expected = hmac.new(key, to_sign.encode(), hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, signature)


def local_path(raw_path: str):
    """The bucket directory and object file for a request path, or None if unsafe."""
    parts = urllib.parse.unquote(raw_path.partition("?")[0]).lstrip("/").split("/", 1)
    bucket = parts[0]
    key = parts[1] if len(parts) > 1 else ""
    if not bucket or bucket in (".", "..") or "\\" in raw_path:
        return None
    if any(segment in ("", ".", "..") for segment in key.split("/")) and key:
        return None
    bucket_dir = os.path.join(ROOT, bucket)
    target = os.path.normpath(os.path.join(bucket_dir, key)) if key else None
    if target is not None and not target.startswith(bucket_dir + os.sep):
        return None
    return bucket_dir, target


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def _answer(self, status: int, body: bytes = b"") -> None:
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        if body:
            self.send_header("Content-Type", "application/octet-stream")
        self.end_headers()
        if body and self.command != "HEAD":
            self.wfile.write(body)
        self.close_connection = True

    def _request(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0) or 0))
        headers = {name.lower(): value for name, value in self.headers.items()}
        if not signature_valid(self.command, self.path, headers, body):
            self._answer(403)
            return None
        paths = local_path(self.path)
        if paths is None:
            self._answer(400)
            return None
        return body, headers, paths

    def do_PUT(self):
        request = self._request()
        if request is None:
            return
        body, headers, (bucket_dir, target) = request
        if target is None:
            if os.path.isdir(bucket_dir):
                return self._answer(409)
            os.makedirs(bucket_dir, exist_ok=True)
            return self._answer(200)
        if not os.path.isdir(bucket_dir):
            return self._answer(404)
        if headers.get("if-none-match") == "*" and os.path.exists(target):
            return self._answer(412)
        os.makedirs(os.path.dirname(target), exist_ok=True)
        temporary = f"{target}.{os.getpid()}.{id(self)}.tmp"
        with open(temporary, "wb") as file:
            file.write(body)
            file.flush()
            os.fsync(file.fileno())
        if headers.get("if-none-match") == "*":
            try:
                os.link(temporary, target)  # fails if another writer won
            except FileExistsError:
                os.unlink(temporary)
                return self._answer(412)
            os.unlink(temporary)
        else:
            os.replace(temporary, target)
        self._answer(200)

    def do_HEAD(self):
        request = self._request()
        if request is None:
            return
        _, _, (bucket_dir, target) = request
        exists = os.path.isdir(bucket_dir) if target is None else os.path.isfile(target)
        self._answer(200 if exists else 404)

    def do_GET(self):
        request = self._request()
        if request is None:
            return
        _, _, (_, target) = request
        if target is None or not os.path.isfile(target):
            return self._answer(404)
        with open(target, "rb") as file:
            self._answer(200, file.read())

    def do_DELETE(self):
        request = self._request()
        if request is None:
            return
        _, _, (_, target) = request
        if target is not None and os.path.isfile(target):
            os.unlink(target)
        self._answer(204)

    def log_message(self, format, *args):
        sys.stderr.write(f"{self.command} {self.path} -> {args[1] if len(args) > 1 else ''}\n")


if __name__ == "__main__":
    os.makedirs(ROOT, exist_ok=True)
    print(f"object store stub on 127.0.0.1:{PORT}, objects under {ROOT}", flush=True)
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
