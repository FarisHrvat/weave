"""Local FTP/FTPS fixture for integration tests. Requires pyftpdlib."""

import argparse
import signal
import sys

from pyftpdlib.authorizers import DummyAuthorizer
from pyftpdlib.handlers import FTPHandler
from pyftpdlib.servers import FTPServer


def with_checksums(base):
    """Adds the XMD5 extension many servers offer, so checksum checks can be tested."""

    class Handler(base):
        proto_cmds = dict(base.proto_cmds)
        proto_cmds["XMD5"] = dict(perm="r", auth=True, arg=True, help="Syntax: XMD5 <SP> file-name")

        def ftp_XMD5(self, path):
            import hashlib

            try:
                with open(path, "rb") as file:
                    digest = hashlib.md5(file.read()).hexdigest()
            except OSError as error:
                self.respond(f"550 {error.strerror}.")
                return
            self.respond(f"250 {digest}")

    return Handler


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True)
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--cert")
    parser.add_argument("--implicit", action="store_true")
    parser.add_argument("--clear-data", action="store_true", help="allow PROT C data connections")
    parser.add_argument("--client-ca", help="require client certificates signed by this PEM")
    args = parser.parse_args()
    if args.implicit and not args.cert:
        parser.error("--implicit requires --cert")

    authorizer = DummyAuthorizer()
    authorizer.add_user("weave", "weave-test", args.root, perm="elradfmwMT")
    if not args.cert:
        authorizer.add_anonymous(args.root, perm="elr")
    if args.cert:
        from pyftpdlib.handlers import TLS_FTPHandler

        if args.implicit:
            class ImplicitTLSHandler(TLS_FTPHandler):
                def handle(self):
                    self.secure_connection(self.ssl_context)

                def handle_ssl_established(self):
                    FTPHandler.handle(self)

            handler = ImplicitTLSHandler
        else:
            handler = TLS_FTPHandler
    else:
        handler = FTPHandler
    handler = with_checksums(handler)
    handler.authorizer = authorizer
    if args.cert:
        handler.certfile = args.cert
        handler.tls_control_required = True
        handler.tls_data_required = not args.clear_data
        if args.client_ca:
            from OpenSSL import SSL

            context = handler.get_ssl_context()
            context.load_verify_locations(args.client_ca)
            context.set_verify(SSL.VERIFY_PEER | SSL.VERIFY_FAIL_IF_NO_PEER_CERT, lambda *_: True)
            handler.ssl_context = context

    server = FTPServer(("127.0.0.1", args.port), handler)
    signal.signal(signal.SIGTERM, lambda _signal, _frame: sys.exit(0))
    try:
        server.serve_forever()
    finally:
        server.close_all()


if __name__ == "__main__":
    main()
