from __future__ import annotations

from contextlib import redirect_stdout
import importlib.util
import io
from pathlib import Path
import sys
import unittest
from unittest.mock import patch


MODULE_PATH = Path(__file__).with_name("launcher.py")
SPEC = importlib.util.spec_from_file_location("demo_launcher", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
launcher = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(launcher)


class LauncherTests(unittest.TestCase):
    def test_bridge_is_ready_only_for_civ_ready(self) -> None:
        self.assertTrue(launcher.bridge_is_ready({"state": "civ_ready"}))
        self.assertFalse(launcher.bridge_is_ready({"state": "connected"}))
        self.assertFalse(launcher.bridge_is_ready(None))

    def test_radio_address_accepts_decimal_and_hexadecimal(self) -> None:
        self.assertEqual(launcher.parse_radio_address("164"), 0xA4)
        self.assertEqual(launcher.parse_radio_address("0xA4"), 0xA4)

    def test_radio_address_rejects_out_of_range_values(self) -> None:
        with self.assertRaises(launcher.argparse.ArgumentTypeError):
            launcher.parse_radio_address("0x100")

    @patch.object(launcher.shutil, "which", return_value=None)
    def test_source_launch_requires_pnpm(self, _which: object) -> None:
        with self.assertRaisesRegex(RuntimeError, "pnpm est introuvable"):
            launcher.source_app_command()

    @patch.object(launcher.shutil, "which", return_value="/tmp/pnpm")
    def test_source_command_uses_pnpm(self, _which: object) -> None:
        self.assertEqual(launcher.source_app_command(), ["/tmp/pnpm", "tauri", "dev"])

    @patch.object(launcher, "run_monitor", return_value=0)
    @patch.object(launcher, "fetch_status", return_value={"state": "civ_ready"})
    @patch.object(launcher, "launch_app")
    def test_main_hands_off_to_monitor_when_bridge_is_ready(
        self,
        launch_app: object,
        _fetch_status: object,
        run_monitor: object,
    ) -> None:
        with patch.object(sys, "argv", ["launcher.py"]):
            with redirect_stdout(io.StringIO()):
                self.assertEqual(launcher.main(), 0)
        launch_app.assert_not_called()
        run_monitor.assert_called_once_with("http://127.0.0.1:8765", None, 160)


if __name__ == "__main__":
    unittest.main()
