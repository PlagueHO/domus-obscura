"""Check the Linux health script with generated loopback HTTP and TLS fixtures."""

from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, HTTPServer
import os
from pathlib import Path
import ssl
import subprocess
import tempfile
from threading import Thread
import unittest


@contextmanager
def listener(status=200, context=None):
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            self.server.requests += 1
            response = status if self.server.requests == 1 else 200
            self.send_response(response)
            if response == 302:
                self.send_header("Location", "/ready")
            self.send_header("Content-Length", "0")
            self.end_headers()

        def log_message(self, *args):
            pass

    server = HTTPServer(("127.0.0.1", 0), Handler)
    server.requests = 0
    if context:
        server.socket = context.wrap_socket(server.socket, server_side=True)
    thread = Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        scheme = "https" if context else "http"
        yield server, f"{scheme}://127.0.0.1:{server.server_port}/"
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        if thread.is_alive():
            raise RuntimeError("health fixture listener did not stop")


def probe(url, **environment):
    configured = {**os.environ, "DOMUS_HEALTH_URL": url}
    configured.pop("DOMUS_HEALTH_CA", None)
    configured.update(environment)
    return subprocess.run(
        ["/bin/sh", str(Path(__file__).with_name("health.sh"))],
        env=configured, capture_output=True, text=True, timeout=8,
    )


class HealthTests(unittest.TestCase):
    def test_browser_requires_http_200_without_proxy_access(self):
        with listener() as (server, url):
            result = probe(url, http_proxy="http://127.0.0.1:1")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(server.requests, 1)
        with listener(503) as (server, url):
            result = probe(url)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("503", result.stderr)

    def test_redirect_is_not_browser_readiness_and_is_not_followed(self):
        with listener(302) as (server, url):
            result = probe(url)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(server.requests, 1)
            self.assertIn("302", result.stderr)

    def test_https_requires_certificate_trust_and_accepts_the_explicit_ca(self):
        with tempfile.TemporaryDirectory(prefix="domus-health-") as directory:
            certificate = Path(directory) / "certificate.pem"
            key = Path(directory) / "key.pem"
            subprocess.run(
                [
                    "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
                    "-subj", "/CN=Domus-health-fixture",
                    "-addext", "subjectAltName=IP:127.0.0.1", "-days", "1",
                    "-keyout", str(key), "-out", str(certificate),
                ],
                check=True, stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE, timeout=15,
            )
            context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            context.minimum_version = ssl.TLSVersion.TLSv1_2
            context.load_cert_chain(certificate, key)
            with listener(context=context) as (server, url):
                untrusted = probe(url)
                self.assertNotEqual(untrusted.returncode, 0)
                self.assertEqual(server.requests, 0)
                trusted = probe(url, DOMUS_HEALTH_CA=str(certificate))
                self.assertEqual(trusted.returncode, 0, trusted.stderr)
                self.assertEqual(server.requests, 1)


if __name__ == "__main__":
    unittest.main()
