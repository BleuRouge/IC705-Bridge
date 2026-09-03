import json
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import ic705bridge as m
from ic705bridge import AUTH_HEADER, BridgeError, IC705Bridge, split_frames, to_hex
from smoke_test import bcd_le, response_frame

seen_headers = {}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send_json(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(body)

    def _send_raw(self, code, text, ctype="application/json"):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.end_headers()
        self.wfile.write(text.encode())

    def do_GET(self):
        seen_headers[self.path] = {k.lower(): v for k, v in self.headers.items()}
        if self.path == "/status":
            self._send_json(200, {"state": "civ_ready", "host": "192.168.1.50"})
        elif self.path == "/stream":
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            for f in ["FE FE E0 A4 03 00 00 50 45 01 FD", "FE FE E0 A4 04 01 02 FD"]:
                self.wfile.write(f"data: {f}\n\n".encode())
                self.wfile.flush()
            self.wfile.write(b": keepalive\n\n")
            self.wfile.flush()
        elif self.path == "/badjson":
            self._send_raw(200, "pas du json")
        elif self.path == "/notdict":
            self._send_raw(200, "[1, 2, 3]")
        elif self.path == "/boom":
            self._send_json(500, {"error": "kaboom"})
        else:
            self._send_json(404, {"error": "not found"})

    def do_POST(self):
        seen_headers[self.path] = {k.lower(): v for k, v in self.headers.items()}
        length = int(self.headers.get("Content-Length", 0))
        self.rfile.read(length)
        self._send_json(200, {"tx": "FE FE A4 E0 03 FD",
                              "response": "FE FE E0 A4 03 00 00 50 45 01 FD"})


class BridgeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.srv = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        cls.port = cls.srv.server_address[1]
        cls.thread = threading.Thread(target=cls.srv.serve_forever, daemon=True)
        cls.thread.start()
        cls.rig = IC705Bridge(f"http://127.0.0.1:{cls.port}")

    @classmethod
    def tearDownClass(cls):
        cls.srv.shutdown()

    # -- to_hex ----------------------------------------------------------
    def test_to_hex_string_tolerant(self):
        self.assertEqual(to_hex("fe fe a4 e0 03 fd"), "FE FE A4 E0 03 FD")
        self.assertEqual(to_hex("0xFE,0xFE-A4:E0;03 FD"), "FE FE A4 E0 03 FD")

    def test_to_hex_bytes_and_ints(self):
        self.assertEqual(to_hex(bytes([0xFE, 0xFE, 0xA4])), "FE FE A4")
        self.assertEqual(to_hex([254, 254, 164]), "FE FE A4")

    def test_to_hex_errors(self):
        with self.assertRaises(BridgeError):
            to_hex("FE F")
        with self.assertRaises(BridgeError):
            to_hex("FE GG")

    # -- split_frames ----------------------------------------------------
    def test_split_frames(self):
        resp = "FE FE A4 E0 03 FD FE FE E0 A4 03 00 00 50 45 01 FD"
        self.assertEqual(
            split_frames(resp),
            ["FE FE A4 E0 03 FD", "FE FE E0 A4 03 00 00 50 45 01 FD"],
        )

    def test_split_frames_ignores_noise_and_empty(self):
        self.assertEqual(split_frames("00 11 FE FE 03 FD 22"), ["FE FE 03 FD"])
        self.assertEqual(split_frames(""), [])

    # -- transport / erreurs --------------------------------------------
    def test_status_and_auth_header(self):
        self.assertEqual(self.rig.status()["state"], "civ_ready")
        self.assertIn(AUTH_HEADER.lower(), seen_headers["/status"])

    def test_send_civ_sends_auth_header(self):
        rep = self.rig.send_civ("FE FE A4 E0 03 FD")
        self.assertIn("response", rep)
        self.assertEqual(seen_headers["/civ"][AUTH_HEADER.lower()], "1")

    def test_is_ready(self):
        self.assertTrue(self.rig.is_ready())

    def test_http_error_normalized(self):
        with self.assertRaises(BridgeError) as ctx:
            self.rig._get("/boom")
        self.assertIn("kaboom", str(ctx.exception))

    def test_bad_json_normalized(self):
        with self.assertRaises(BridgeError):
            self.rig._get("/badjson")

    def test_non_object_json_normalized(self):
        with self.assertRaises(BridgeError):
            self.rig._get("/notdict")

    def test_unreachable_normalized(self):
        with self.assertRaises(BridgeError):
            IC705Bridge("http://127.0.0.1:1", timeout=1).status()

    # -- stream ----------------------------------------------------------
    def test_stream_civ(self):
        got = []
        for fr in self.rig.stream_civ(timeout=3):
            got.append(fr)
            if len(got) == 2:
                break
        self.assertEqual(
            got, ["FE FE E0 A4 03 00 00 50 45 01 FD", "FE FE E0 A4 04 01 02 FD"]
        )
        self.assertEqual(seen_headers["/stream"][AUTH_HEADER.lower()], "1")

    def test_version(self):
        self.assertEqual(m.__version__, "0.1.1")

    # -- smoke test réel (parsers testables sans radio) -----------------
    def test_smoke_response_parser(self):
        response = "FE FE E0 A4 03 00 00 50 45 01 FD"
        frame = response_frame(response, 0x03, 5)
        self.assertEqual(bcd_le(frame[5:10]), 145_500_000)

    def test_smoke_response_parser_rejects_wrong_command(self):
        with self.assertRaises(BridgeError):
            response_frame("FE FE E0 A4 04 01 02 FD", 0x03, 5)


if __name__ == "__main__":
    unittest.main()
