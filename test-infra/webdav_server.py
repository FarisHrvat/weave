"""Small authenticated WebDAV fixture for local integration tests."""

import argparse
import base64
import html
import os
import ssl
from email.utils import formatdate
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import unquote, urlsplit


def handler_for(root: Path):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def authorize(self):
            expected = "Basic " + base64.b64encode(b"weave:weave-test").decode()
            if self.headers.get("Authorization") == expected:
                return True
            self.send_response(401)
            self.send_header("WWW-Authenticate", 'Basic realm="Weave test"')
            self.send_header("Content-Length", "0")
            self.end_headers()
            return False

        def target(self):
            path = unquote(urlsplit(self.path).path)
            if not path.startswith("/dav/"):
                return None
            target = (root / path.removeprefix("/dav/")).resolve()
            if target != root and root not in target.parents:
                return None
            return target

        def reply(self, status, data=b"", content_type=None, extra=None):
            self.send_response(status)
            self.send_header("Content-Length", str(len(data)))
            if content_type:
                self.send_header("Content-Type", content_type)
            for key, value in (extra or {}).items():
                self.send_header(key, value)
            self.end_headers()
            if data:
                self.wfile.write(data)

        def do_PROPFIND(self):
            if not self.authorize():
                return
            self.rfile.read(int(self.headers.get("Content-Length", "0")))
            target = self.target()
            if target is None or not target.exists():
                self.reply(404)
                return
            depth = self.headers.get("Depth", "1")
            items = [target]
            if depth == "1" and target.is_dir():
                items.extend(sorted(target.iterdir()))
            parts = ['<?xml version="1.0"?><d:multistatus xmlns:d="DAV:">']
            for item in items:
                relative = item.relative_to(root).parts
                from urllib.parse import quote
                href = "/dav/" + "/".join(quote(part, safe="") for part in relative)
                if item.is_dir() and not href.endswith("/"):
                    href += "/"
                kind = "<d:collection/>" if item.is_dir() else ""
                length = 0 if item.is_dir() else item.stat().st_size
                modified = formatdate(item.stat().st_mtime, usegmt=True)
                parts.append(f"<d:response><d:href>{html.escape(href)}</d:href><d:propstat><d:prop><d:resourcetype>{kind}</d:resourcetype><d:getcontentlength>{length}</d:getcontentlength><d:getlastmodified>{modified}</d:getlastmodified></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>")
            parts.append("</d:multistatus>")
            self.reply(207, "".join(parts).encode(), "application/xml")

        def do_GET(self):
            if not self.authorize():
                return
            target = self.target()
            if target is None or not target.is_file():
                self.reply(404)
                return
            data = target.read_bytes()
            range_header = self.headers.get("Range")
            if range_header:
                offset = int(range_header.removeprefix("bytes=").removesuffix("-"))
                if offset >= len(data):
                    self.reply(416)
                    return
                self.reply(206, data[offset:], extra={"Content-Range": f"bytes {offset}-{len(data)-1}/{len(data)}"})
            else:
                self.reply(200, data)

        def do_PUT(self):
            if not self.authorize():
                return
            target = self.target()
            if target is None or not target.parent.is_dir():
                self.reply(409)
                return
            length = int(self.headers.get("Content-Length", "0"))
            target.write_bytes(self.rfile.read(length))
            modified = self.headers.get("X-OC-MTime")
            if modified:
                try:
                    os.utime(target, (int(modified), int(modified)))
                except (ValueError, OverflowError, OSError):
                    self.reply(400)
                    return
                self.reply(201, extra={"X-OC-MTime": "accepted"})
            else:
                self.reply(201)

        def do_MKCOL(self):
            if not self.authorize():
                return
            target = self.target()
            if target is None or not target.parent.is_dir():
                self.reply(409)
                return
            if target.exists():
                self.reply(405)
                return
            target.mkdir()
            self.reply(201)

        def do_MOVE(self):
            if not self.authorize():
                return
            source = self.target()
            destination = unquote(urlsplit(self.headers.get("Destination", "")).path)
            if source is None or not source.exists() or not destination.startswith("/dav/"):
                self.reply(404)
                return
            target = (root / destination.removeprefix("/dav/")).resolve()
            if target != root and root not in target.parents or not target.parent.is_dir():
                self.reply(409)
                return
            if target.exists():
                self.reply(412)
                return
            source.rename(target)
            self.reply(201)

        def do_DELETE(self):
            if not self.authorize():
                return
            target = self.target()
            if target is None or target == root or not target.exists():
                self.reply(404)
                return
            if target.is_dir():
                import shutil
                shutil.rmtree(target)
            else:
                target.unlink()
            self.reply(204)

        def log_message(self, _format, *_args):
            pass

    return Handler


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--cert")
    parser.add_argument("--key")
    args = parser.parse_args()
    root = Path(args.root).resolve()
    root.mkdir(parents=True, exist_ok=True)
    server = ThreadingHTTPServer(("127.0.0.1", args.port), handler_for(root))
    if args.cert and args.key:
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(args.cert, args.key)
        server.socket = context.wrap_socket(server.socket, server_side=True)
    try:
        server.serve_forever()
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
