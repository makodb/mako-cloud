#!/usr/bin/env python3
"""A local SMTP server that keeps every message, for Mako development.

The control plane sends developer and application mail (sign-up, password
reset, magic links, invitations) through lettre with AUTH PLAIN or LOGIN over
plaintext on loopback. This accepts that session, checks the password against
the file the control plane is configured with, and writes each message to
MAILDIR as a .eml file. Nothing is relayed anywhere.

It also serves a read-only inbox on the HTTP port: / lists messages newest
first, /<name> shows one with its links made clickable.

Usage: mail-sink.py <smtp-port> <http-port> <maildir> <username> <password-file>
(scripts/local/dev-services.sh starts it with the values in .env.)
"""
import base64
import email
import email.policy
import html
import os
import re
import socketserver
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SMTP_PORT, HTTP_PORT, MAILDIR = int(sys.argv[1]), int(sys.argv[2]), os.path.abspath(sys.argv[3])
USERNAME, PASSWORD = sys.argv[4], open(sys.argv[5]).read().strip()
MAX_MESSAGE_BYTES = 10 * 1024 * 1024
counter = 0
counter_lock = threading.Lock()


class Session(socketserver.StreamRequestHandler):
    def reply(self, text: str) -> None:
        self.wfile.write(text.encode() + b"\r\n")

    def line(self) -> str:
        return self.rfile.readline(65536).decode(errors="replace").rstrip("\r\n")

    def handle(self):
        authenticated, sender, recipients = False, None, []
        self.reply("220 mako-mail-sink ESMTP")
        while True:
            command = self.line()
            if not command:
                return
            verb = command.split(" ", 1)[0].upper()
            argument = command[len(verb):].strip()
            if verb in ("EHLO", "HELO"):
                self.reply("250-mako-mail-sink\r\n250-AUTH PLAIN LOGIN\r\n250-8BITMIME\r\n250 SIZE 10485760"
                           if verb == "EHLO" else "250 mako-mail-sink")
            elif verb == "AUTH":
                mechanism, _, initial = argument.partition(" ")
                if mechanism.upper() == "PLAIN":
                    if not initial:
                        self.reply("334 ")
                        initial = self.line()
                    try:
                        _, user, password = base64.b64decode(initial).decode().split("\0")
                    except (ValueError, UnicodeDecodeError):
                        user, password = "", ""
                elif mechanism.upper() == "LOGIN":
                    self.reply("334 VXNlcm5hbWU6")
                    user = base64.b64decode(self.line() or "").decode(errors="replace")
                    self.reply("334 UGFzc3dvcmQ6")
                    password = base64.b64decode(self.line() or "").decode(errors="replace")
                else:
                    self.reply("504 unrecognized authentication type")
                    continue
                authenticated = user == USERNAME and password == PASSWORD
                self.reply("235 authenticated" if authenticated else "535 authentication failed")
            elif verb == "MAIL":
                if not authenticated:
                    self.reply("530 authentication required")
                    continue
                sender, recipients = argument, []
                self.reply("250 OK")
            elif verb == "RCPT":
                recipients.append(argument)
                self.reply("250 OK")
            elif verb == "DATA":
                if sender is None or not recipients:
                    self.reply("503 need MAIL and RCPT first")
                    continue
                self.reply("354 end with <CRLF>.<CRLF>")
                lines, size = [], 0
                while True:
                    raw = self.rfile.readline(1024 * 1024)
                    if not raw or raw in (b".\r\n", b".\n"):
                        break
                    size += len(raw)
                    if size > MAX_MESSAGE_BYTES:
                        self.reply("552 message too large")
                        return
                    lines.append(raw[1:] if raw.startswith(b"..") else raw)
                save(b"".join(lines), sender, recipients)
                sender, recipients = None, []
                self.reply("250 queued")
            elif verb == "RSET":
                sender, recipients = None, []
                self.reply("250 OK")
            elif verb == "NOOP":
                self.reply("250 OK")
            elif verb == "QUIT":
                self.reply("221 bye")
                return
            else:
                self.reply("502 command not implemented")


def save(message: bytes, sender: str, recipients: list) -> None:
    global counter
    with counter_lock:
        counter += 1
        name = f"{time.strftime('%Y%m%dT%H%M%S')}-{counter:04d}.eml"
    envelope = f"X-Envelope-From: {sender}\r\nX-Envelope-To: {', '.join(recipients)}\r\n".encode()
    with open(os.path.join(MAILDIR, name), "wb") as file:
        file.write(envelope + message)
    parsed = email.message_from_bytes(message, policy=email.policy.default)
    print(f"stored {name}: to={', '.join(recipients)} subject={parsed['subject']!r}", flush=True)


def text_of(parsed) -> str:
    part = parsed.get_body(preferencelist=("plain", "html"))
    return part.get_content() if part is not None else ""


class Inbox(BaseHTTPRequestHandler):
    def do_GET(self):
        names = sorted((n for n in os.listdir(MAILDIR) if n.endswith(".eml")), reverse=True)
        if self.path in ("/", ""):
            rows = []
            for name in names[:200]:
                with open(os.path.join(MAILDIR, name), "rb") as file:
                    parsed = email.message_from_bytes(file.read(), policy=email.policy.default)
                rows.append(f"<tr><td>{html.escape(name[:15])}</td><td>{html.escape(str(parsed['to']))}</td>"
                            f"<td><a href='/{html.escape(name)}'>{html.escape(str(parsed['subject']))}</a></td></tr>")
            body = ("<h1>Local mail</h1><p>Messages the control plane sent; nothing leaves this host.</p>"
                    "<table><tr><th>Received</th><th>To</th><th>Subject</th></tr>" + "".join(rows) + "</table>")
        elif self.path.lstrip("/") in names:
            with open(os.path.join(MAILDIR, self.path.lstrip("/")), "rb") as file:
                parsed = email.message_from_bytes(file.read(), policy=email.policy.default)
            text = html.escape(text_of(parsed))
            text = re.sub(r"(https?://[^\s<]+)", r"<a href='\1'>\1</a>", text)
            body = (f"<p><a href='/'>&larr; inbox</a></p><h1>{html.escape(str(parsed['subject']))}</h1>"
                    f"<p>To {html.escape(str(parsed['to']))} &middot; from {html.escape(str(parsed['from']))}</p>"
                    f"<pre style='white-space:pre-wrap'>{text}</pre>")
        else:
            self.send_error(404)
            return
        page = ("<!doctype html><meta charset=utf-8><title>Local mail</title>"
                "<style>body{font:14px system-ui;margin:2rem}td,th{padding:4px 12px;text-align:left}</style>"
                + body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(page)))
        self.end_headers()
        self.wfile.write(page)

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    os.makedirs(MAILDIR, exist_ok=True)
    socketserver.ThreadingTCPServer.allow_reuse_address = True
    smtp = socketserver.ThreadingTCPServer(("127.0.0.1", SMTP_PORT), Session)
    threading.Thread(target=smtp.serve_forever, daemon=True).start()
    print(f"mail sink: smtp 127.0.0.1:{SMTP_PORT}, inbox http://127.0.0.1:{HTTP_PORT}, stored in {MAILDIR}", flush=True)
    ThreadingHTTPServer(("127.0.0.1", HTTP_PORT), Inbox).serve_forever()
