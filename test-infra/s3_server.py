"""Disposable S3-compatible server that verifies request signatures for tests."""

import argparse
import base64
import hashlib
import hmac
from datetime import datetime, timedelta, timezone
import html
import xml.etree.ElementTree as ET
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Lock
from urllib.parse import parse_qs, quote, unquote, urlsplit


ACCESS = "AKIDEXAMPLE"
TEMP_ACCESS = "AKIDTEMP"
SECRET = "weave-test-secret"
TOKEN = "weave+token/2026=temporary"
REGION = "us-east-1"
BUCKET = "weave-test"


def signed(key, value):
    return hmac.new(key, value.encode(), hashlib.sha256).digest()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    objects = {f"paged/item-{index:04}.txt": b"x" for index in range(1005)}
    uploads = {}
    properties = {}
    tags = {}
    next_upload = 0
    flaky_failures = 0
    lock = Lock()

    def respond(self, code, body=b"", headers=None):
        self.send_response(code)
        self.send_header("Content-Length", str(len(body)))
        for name, value in (headers or {}).items():
            self.send_header(name, value)
        self.end_headers()
        if self.command != "HEAD" and body:
            self.wfile.write(body)

    def authorized(self, body):
        if "X-Amz-Signature=" in self.path:
            return self.authorized_share_link(body)
        header = self.headers.get("Authorization", "")
        if not header.startswith("AWS4-HMAC-SHA256 "):
            return False
        fields = dict(part.split("=", 1) for part in header.removeprefix("AWS4-HMAC-SHA256 ").split(",") if "=" in part)
        credential = fields.get("Credential", "").split("/")
        if len(credential) != 5 or credential[0] not in (ACCESS, TEMP_ACCESS) or credential[2:] != [REGION, "s3", "aws4_request"]:
            return False
        date = credential[1]
        timestamp = self.headers.get("x-amz-date", "")
        if not timestamp.startswith(date):
            return False
        payload_hash = self.headers.get("x-amz-content-sha256", "")
        if payload_hash != hashlib.sha256(body).hexdigest():
            return False
        names = fields.get("SignedHeaders", "").split(";")
        if names != sorted(names) or any(not self.headers.get(name) for name in names):
            return False
        if credential[0] == TEMP_ACCESS and (self.headers.get("x-amz-security-token") != TOKEN or "x-amz-security-token" not in names):
            return False
        parts = urlsplit(self.path)
        canonical_headers = "".join(f"{name}:{self.headers[name].strip()}\n" for name in names)
        canonical = "\n".join([self.command, parts.path, parts.query, canonical_headers, ";".join(names), payload_hash])
        scope = f"{date}/{REGION}/s3/aws4_request"
        string_to_sign = "\n".join(["AWS4-HMAC-SHA256", timestamp, scope, hashlib.sha256(canonical.encode()).hexdigest()])
        key = signed(("AWS4" + SECRET).encode(), date)
        key = signed(key, REGION)
        key = signed(key, "s3")
        key = signed(key, "aws4_request")
        expected = hmac.new(key, string_to_sign.encode(), hashlib.sha256).hexdigest()
        return hmac.compare_digest(expected, fields.get("Signature", ""))

    def authorized_share_link(self, body):
        if self.command != "GET" or body:
            return False
        parts = urlsplit(self.path)
        query = parse_qs(parts.query)
        if query.get("X-Amz-Algorithm") != ["AWS4-HMAC-SHA256"] or query.get("X-Amz-SignedHeaders") != ["host"]:
            return False
        credential = query.get("X-Amz-Credential", [""])[0].split("/")
        if len(credential) != 5 or credential[0] not in (ACCESS, TEMP_ACCESS) or credential[2:] != [REGION, "s3", "aws4_request"]:
            return False
        if credential[0] == TEMP_ACCESS and query.get("X-Amz-Security-Token") != [TOKEN]:
            return False
        date = credential[1]
        timestamp = query.get("X-Amz-Date", [""])[0]
        try:
            issued = datetime.strptime(timestamp, "%Y%m%dT%H%M%SZ").replace(tzinfo=timezone.utc)
            lifetime = int(query.get("X-Amz-Expires", [""])[0])
        except ValueError:
            return False
        if not 60 <= lifetime <= 604800 or datetime.now(timezone.utc) > issued + timedelta(seconds=lifetime):
            return False
        if not parts.query.endswith(query.get("X-Amz-Signature", [""])[0]):
            return False
        canonical_query = parts.query.rsplit("&X-Amz-Signature=", 1)[0]
        canonical = "\n".join(["GET", parts.path, canonical_query, f"host:{self.headers['Host'].strip()}\n", "host", "UNSIGNED-PAYLOAD"])
        scope = f"{date}/{REGION}/s3/aws4_request"
        string_to_sign = "\n".join(["AWS4-HMAC-SHA256", timestamp, scope, hashlib.sha256(canonical.encode()).hexdigest()])
        key = signed(("AWS4" + SECRET).encode(), date)
        key = signed(key, REGION)
        key = signed(key, "s3")
        key = signed(key, "aws4_request")
        expected = hmac.new(key, string_to_sign.encode(), hashlib.sha256).hexdigest()
        return hmac.compare_digest(expected, query.get("X-Amz-Signature", [""])[0])

    def stored_headers(self):
        names = ("content-type", "cache-control", "content-disposition", "content-encoding", "x-amz-storage-class")
        return {
            name: value
            for name, value in ((name.lower(), value) for name, value in self.headers.items())
            if name in names or name.startswith("x-amz-meta-")
        }

    def route(self):
        parts = urlsplit(self.path)
        # Virtual-hosted requests name the bucket in the Host header.
        virtual = self.headers.get("Host", "").startswith(f"{BUCKET}.")
        prefix = "/" if virtual else f"/{BUCKET}/"
        if not parts.path.startswith(prefix):
            return None, None
        return unquote(parts.path[len(prefix):]), parse_qs(parts.query, keep_blank_values=True)

    def handle_request(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        if not self.authorized(body):
            self.respond(403)
            return
        key, query = self.route()
        if key is None:
            self.respond(404)
            return
        if self.command == "GET" and query.get("list-type") == ["2"]:
            self.list_objects(query)
            return
        with self.lock:
            if self.command == "GET" and key == "" and "uploads" in query:
                prefix = query.get("prefix", [""])[0]
                found = [(upload_id, upload) for upload_id, upload in self.uploads.items() if upload["key"].startswith(prefix)]
                listed = "".join(
                    f"<Upload><Key>{html.escape(upload['key'])}</Key><UploadId>{upload_id}</UploadId></Upload>" for upload_id, upload in found
                )
                self.respond(200, f"<ListMultipartUploadsResult><Count>{len(found)}</Count>{listed}</ListMultipartUploadsResult>".encode())
                return
            if self.command == "POST" and "uploads" in query:
                type(self).next_upload += 1
                upload_id = f"weave-test-{self.next_upload}"
                self.uploads[upload_id] = {"key": key, "parts": {}}
                self.respond(200, f"<InitiateMultipartUploadResult><UploadId>{upload_id}</UploadId></InitiateMultipartUploadResult>".encode())
                return
            if "tagging" in query:
                if key not in self.objects:
                    self.respond(404)
                    return
                if self.command == "GET":
                    listed = "".join(
                        f"<Tag><Key>{html.escape(name)}</Key><Value>{html.escape(value)}</Value></Tag>"
                        for name, value in self.tags.get(key, [])
                    )
                    self.respond(200, f"<Tagging><TagSet>{listed}</TagSet></Tagging>".encode())
                    return
                if self.command == "PUT":
                    digest = self.headers.get("Content-MD5")
                    if digest and digest != base64.b64encode(hashlib.md5(body).digest()).decode():
                        self.respond(400)
                        return
                    try:
                        root = ET.fromstring(body)
                        self.tags[key] = [(tag.findtext("Key"), tag.findtext("Value") or "") for tag in root.iter("Tag")]
                    except ET.ParseError:
                        self.respond(400)
                        return
                    self.respond(200)
                    return
            upload_id = query.get("uploadId", [None])[0]
            if upload_id:
                upload = self.uploads.get(upload_id)
                if not upload or upload["key"] != key:
                    self.respond(404)
                    return
                if self.command == "DELETE":
                    del self.uploads[upload_id]
                    self.respond(204)
                    return
                if self.command == "GET":
                    listed = "".join(
                        f"<Part><PartNumber>{number}</PartNumber><ETag>{html.escape(etag)}</ETag><Size>{len(data)}</Size></Part>"
                        for number, (etag, data) in sorted(upload["parts"].items())
                    )
                    self.respond(200, f"<ListPartsResult><IsTruncated>false</IsTruncated>{listed}</ListPartsResult>".encode())
                    return
                if self.command == "PUT" and "partNumber" in query:
                    number = int(query["partNumber"][0])
                    if key == "fail-part.bin" and number == 2:
                        self.respond(500)
                        return
                    if key == "flaky-part.bin":
                        if number == 1 and number in upload["parts"]:
                            self.respond(409)
                            return
                        if number == 2 and type(self).flaky_failures == 0:
                            type(self).flaky_failures += 1
                            self.respond(503)
                            return
                    copy_source = self.headers.get("x-amz-copy-source")
                    if copy_source:
                        old = unquote(copy_source).removeprefix(f"/{BUCKET}/")
                        first, last = map(int, self.headers.get("x-amz-copy-source-range", "").removeprefix("bytes=").split("-"))
                        body = self.objects[old][first:last + 1]
                        etag = f'"{hashlib.md5(body).hexdigest()}"'
                        upload["parts"][number] = (etag, body)
                        self.respond(200, f"<CopyPartResult><ETag>{html.escape(etag)}</ETag></CopyPartResult>".encode())
                        return
                    etag = f'"{hashlib.md5(body).hexdigest()}"'
                    upload["parts"][number] = (etag, body)
                    self.respond(200, headers={"ETag": etag})
                    return
                if self.command == "POST":
                    if key == "fail-complete.bin":
                        self.respond(200, b"<Error><Code>InternalError</Code></Error>")
                        return
                    try:
                        root = ET.fromstring(body)
                        requested = [(int(part.findtext("PartNumber")), part.findtext("ETag")) for part in root.findall("Part")]
                    except (ET.ParseError, TypeError, ValueError):
                        self.respond(400)
                        return
                    if not requested or requested != [(number, upload["parts"][number][0]) for number, _ in requested if number in upload["parts"]]:
                        self.respond(400)
                        return
                    if any(len(upload["parts"][number][1]) < 5 * 1024 * 1024 for number, _ in requested[:-1]):
                        self.respond(400)
                        return
                    self.objects[key] = b"".join(upload["parts"][number][1] for number, _ in requested)
                    del self.uploads[upload_id]
                    self.respond(200, b"<CompleteMultipartUploadResult><ETag>complete</ETag></CompleteMultipartUploadResult>")
                    return
            if self.command == "PUT":
                copy_source = self.headers.get("x-amz-copy-source")
                if copy_source:
                    old = unquote(copy_source).removeprefix(f"/{BUCKET}/")
                    if old not in self.objects:
                        self.respond(404)
                        return
                    if old == key and self.headers.get("x-amz-metadata-directive") != "REPLACE":
                        self.respond(400)
                        return
                    self.objects[key] = self.objects[old]
                    if self.headers.get("x-amz-metadata-directive") == "REPLACE":
                        self.properties[key] = self.stored_headers()
                    else:
                        self.properties[key] = dict(self.properties.get(old, {}))
                    self.tags[key] = list(self.tags.get(old, []))
                    self.respond(200, b"<CopyObjectResult><ETag>test</ETag></CopyObjectResult>")
                    return
                else:
                    self.objects[key] = body
                    self.properties[key] = self.stored_headers()
                self.respond(200)
            elif self.command == "HEAD":
                if key not in self.objects:
                    self.respond(404)
                    return
                headers = {"ETag": f'"{hashlib.md5(self.objects[key]).hexdigest()}"', "Last-Modified": "Mon, 01 Jan 2024 00:00:00 GMT"}
                headers.update(self.properties.get(key, {}))
                self.respond(200, self.objects[key], headers)
            elif self.command == "DELETE":
                self.objects.pop(key, None)
                self.properties.pop(key, None)
                self.tags.pop(key, None)
                self.respond(204)
            elif self.command == "GET":
                if key not in self.objects:
                    self.respond(404)
                    return
                data = self.objects[key]
                range_header = self.headers.get("Range")
                if range_header:
                    offset = int(range_header.removeprefix("bytes=").removesuffix("-"))
                    if offset >= len(data):
                        self.respond(416)
                        return
                    self.respond(206, data[offset:], {"Content-Range": f"bytes {offset}-{len(data)-1}/{len(data)}"})
                else:
                    self.respond(200, data)
            else:
                self.respond(405)

    def list_objects(self, query):
        prefix = query.get("prefix", [""])[0]
        delimiter = query.get("delimiter", [None])[0]
        start = int(query.get("continuation-token", ["0"])[0])
        limit = min(int(query.get("max-keys", ["1000"])[0]), 1000)
        with self.lock:
            keys = sorted(key for key in self.objects if key.startswith(prefix))
            items = []
            seen = set()
            for key in keys:
                remainder = key[len(prefix):]
                if delimiter and delimiter in remainder:
                    common = prefix + remainder.split(delimiter, 1)[0] + delimiter
                    if common not in seen:
                        items.append(("CommonPrefixes", common))
                        seen.add(common)
                else:
                    items.append(("Contents", key))
            page = items[start:start + limit]
            next_token = start + limit if start + limit < len(items) else None
            encoded = query.get("encoding-type") == ["url"]
            xml = ["<ListBucketResult>"]
            if encoded:
                xml.append("<EncodingType>url</EncodingType>")
            for kind, key in page:
                shown = quote(key, safe="/") if encoded else key
                if kind == "Contents":
                    xml.append(f"<Contents><Key>{html.escape(shown)}</Key><Size>{len(self.objects[key])}</Size><LastModified>2024-01-01T00:00:00.000Z</LastModified></Contents>")
                else:
                    xml.append(f"<CommonPrefixes><Prefix>{html.escape(shown)}</Prefix></CommonPrefixes>")
            if next_token is not None:
                xml.append(f"<NextContinuationToken>{next_token}</NextContinuationToken>")
            xml.append("</ListBucketResult>")
        self.respond(200, "".join(xml).encode(), {"Content-Type": "application/xml"})

    do_GET = handle_request
    do_PUT = handle_request
    do_POST = handle_request
    do_DELETE = handle_request
    do_HEAD = handle_request

    def log_message(self, _format, *_args):
        pass


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args()
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    try:
        server.serve_forever()
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
