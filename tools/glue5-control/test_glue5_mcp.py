import json
import unittest
from unittest.mock import patch
from glue5_mcp import dispatch, OPERATIONS


class McpTests(unittest.TestCase):
    def test_initialize(self):
        result = dispatch({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                           "params": {"protocolVersion": "2025-06-18"}}, port=49355, token="")
        self.assertEqual(result["result"]["serverInfo"]["name"], "glue5-diagnostic")

    def test_tools_are_strictly_allowlisted(self):
        r = dispatch({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}, port=49355, token="")
        names = [t["name"] for t in r["result"]["tools"]]
        self.assertEqual(set(names), set(OPERATIONS))
        self.assertFalse(any("shell" in name or "file" in name for name in names))

    def test_mcp_forwards_only_named_operations(self):
        with patch("glue5_mcp.request", return_value={"ready": True}) as call:
            r = dispatch({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                          "params": {"name": "glue5_status", "arguments": {}}},
                         port=100, token="testtoken")
        call.assert_called_once_with(100, "testtoken", "status", {})
        self.assertTrue(json.loads(r["result"]["content"][0]["text"])["ready"])

    def test_unknown_tool_never_reaches_transport(self):
        with patch("glue5_mcp.request") as call:
            r = dispatch({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
                          "params": {"name": "exec_shell", "arguments": {"command": "x"}}},
                         port=100, token="testtoken")
        call.assert_not_called()
        self.assertTrue(r["result"]["isError"])

    def test_notifications_do_not_get_responses(self):
        self.assertIsNone(dispatch({"jsonrpc": "2.0", "method": "notifications/initialized"},
                                   port=100, token="testtoken"))


if __name__ == "__main__":
    unittest.main()
