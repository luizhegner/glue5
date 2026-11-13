"""Protocol framing and client behavior without starting GLUE5 or the renderer."""
import json
import socket
import threading
import unittest

from glue5_control import ControlError, request


class MockDiagnostic:
    def __init__(self, response):
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.sock.bind(("127.0.0.1", 0))
        self.sock.listen(1)
        self.port = self.sock.getsockname()[1]
        self.received = None
        self.response = response

    def __enter__(self):
        def server():
            with self.sock:
                conn, _ = self.sock.accept()
                with conn:
                    wire = bytearray()
                    while not wire.endswith(b"\n"):
                        wire.extend(conn.recv(1024))
                    self.received = json.loads(wire)
                    conn.sendall((json.dumps(self.response) + "\n").encode())
        self.thread = threading.Thread(target=server, daemon=True)
        self.thread.start()
        return self

    def __exit__(self, *args):
        self.thread.join(timeout=2)
        self.sock.close()


class ClientTests(unittest.TestCase):
    def test_status_with_authenticated_envelope(self):
        with MockDiagnostic({"v": 1, "id": "cli", "ok": True, "result": {"runtime_input_ready": True}}) as m:
            result = request(m.port, "a" * 24, "status")
        self.assertTrue(result["runtime_input_ready"])
        self.assertEqual(m.received["v"], 1)
        self.assertEqual(m.received["token"], "a" * 24)
        self.assertEqual(m.received["op"], "status")
        self.assertEqual(m.received["args"], {})

    def test_server_rejection_is_not_ignored(self):
        with MockDiagnostic({"v": 1, "id": "cli", "ok": False, "error": "control not in allowlist"}) as m:
            with self.assertRaisesRegex(ControlError, "allowlist"):
                request(m.port, "a" * 24, "input_press", {"control": "invalid"})

    def test_wrong_version_rejected(self):
        with MockDiagnostic({"v": 2, "id": "cli", "ok": True, "result": {}}) as m:
            with self.assertRaisesRegex(ControlError, "protocol version"):
                request(m.port, "a" * 24, "status")

    def test_missing_token_rejected_without_connecting(self):
        with self.assertRaisesRegex(ControlError, "missing or too short"):
            request(49355, "", "status")

    def test_oversized_request_rejected_without_connecting(self):
        with self.assertRaisesRegex(ControlError, "exceeds protocol limit"):
            request(49355, "x" * 24, "status", {"payload": "x" * 8192})


if __name__ == "__main__":
    unittest.main()
