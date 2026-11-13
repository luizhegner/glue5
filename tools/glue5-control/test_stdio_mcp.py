import json
import pathlib
import subprocess
import sys
import unittest


class McpStdioTests(unittest.TestCase):
    def test_stdio_initialization_is_valid_jsonrpc(self):
        adapter = pathlib.Path(__file__).with_name("glue5_mcp.py")
        message = {"jsonrpc": "2.0", "id": 15, "method": "initialize",
                   "params": {"protocolVersion": "2025-06-18"}}
        process = subprocess.run(
            [sys.executable, str(adapter)],
            input=json.dumps(message) + "\n",
            text=True,
            encoding="utf-8",
            capture_output=True,
            timeout=5,
            check=True,
        )
        self.assertFalse(process.stderr)
        response = json.loads(process.stdout)
        self.assertEqual(response["id"], 15)
        self.assertEqual(response["result"]["protocolVersion"], "2025-06-18")


if __name__ == "__main__":
    unittest.main()
