"""Contract checks for the bridge; build tools/shitty-adapter before running."""

import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

import adapter
import run

ROOT = Path(__file__).resolve().parents[2]
BINARY = ROOT / "target/shitty-adapter/debug" / (
    "zetta-shitty-adapter.exe" if sys.platform == "win32" else "zetta-shitty-adapter")


class AdapterTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not BINARY.is_file():
            raise RuntimeError("build tools/shitty-adapter into target/shitty-adapter first")
        sys.path.insert(0, str(ROOT / "shitty/tst"))
        import harness
        adapter.install(harness, BINARY)

    def test_fragmented_utf8_and_escape_sequences_match_whole_write(self):
        data = "\x1b[2;3H\x1b[1;38;2;1;2;3m界e\u0301".encode()
        with adapter.Zetta(columns=12, rows=3) as whole, adapter.Zetta(columns=12, rows=3) as split:
            whole.write(data)
            split.write_chunks(*(bytes([byte]) for byte in data))
            self.assertEqual(whole.snapshot(), split.snapshot())
            cell = split.snapshot().cell(2, 1)
            self.assertTrue(cell.double_width)
            self.assertTrue(cell.bold)
            self.assertEqual(cell.foreground, (1, 2, 3))

    def test_device_response_is_drained_once(self):
        with adapter.Zetta(columns=8, rows=3) as terminal:
            terminal.write(b"\x1b[2;3H\x1b[6n")
            self.assertEqual(terminal.read_input(), b"\x1b[2;3R")
            self.assertEqual(terminal.read_input(), b"")

    def test_frontend_dependent_replies_skip_instead_of_returning_fake_bytes(self):
        with adapter.Zetta(columns=8, rows=2) as terminal:
            terminal.write(b"\x1b]10;?\x07")
            with self.assertRaisesRegex(unittest.SkipTest, "frontend color reply"):
                terminal.read_input()

    def test_resize_and_scrollback_observe_actual_grid(self):
        with adapter.Zetta(columns=8, rows=2) as terminal:
            terminal.write(b"first\r\nsecond\r\nthird")
            terminal.wheel_up()
            self.assertEqual(terminal.snapshot().lines, ["first   ", "second  "])
            terminal.wheel_down()
            self.assertEqual(terminal.snapshot().lines, ["second  ", "third   "])
            terminal.resize(10, 3)
            self.assertEqual((terminal.snapshot().columns, terminal.snapshot().rows), (10, 3))
            with self.assertRaises(RuntimeError):
                terminal.resize(0, 3)
            self.assertEqual(terminal.snapshot().columns, 10)

    def test_missing_observations_and_operations_are_explicit_skips(self):
        with adapter.Zetta(columns=8, rows=2) as terminal:
            with self.assertRaisesRegex(unittest.SkipTest, "semantic"):
                _ = terminal.snapshot().cell(0, 0).semantic
            with self.assertRaisesRegex(unittest.SkipTest, "spawn"):
                terminal.spawn("sh")
            with self.assertRaises(AttributeError):
                terminal.misspelled_operation()
        with self.assertRaisesRegex(unittest.SkipTest, "extra_arguments"):
            adapter.Zetta(extra_arguments=("-fg", "red"))

    def test_tab_copy_markers_are_blank_in_visual_snapshots(self):
        with adapter.Zetta(columns=10, rows=2) as terminal:
            terminal.write(b"A\tB")
            snapshot = terminal.snapshot()
            self.assertEqual(snapshot.lines[0], "A       B ")
            self.assertEqual(snapshot.cell(1, 0).char, " ")
        run.replay_case(ROOT / "shitty/tst/xtermjs/t0501-bash_ls.in")

    def test_recorded_shell_redraw_runs_through_engine(self):
        run.replay_case(ROOT / "shitty/tst/xtermjs/t0500-bash_long_line.in")

    def test_assertion_failure_is_reported_and_returns_nonzero(self):
        # Use a temporary imported-style test so this stays valid if the engine
        # later fixes every currently failing upstream conformance assertion.
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / "bridge_failure.py").write_text(
                "import unittest\nfrom harness import Shitty\n"
                "class DeliberateFailure(unittest.TestCase):\n"
                "    def test_failure(self):\n"
                "        with Shitty(columns=8, rows=2) as terminal:\n"
                "            terminal.write(b'actual')\n"
                "            self.assertEqual(terminal.snapshot().lines[0], 'wrong')\n")
            import os
            environment = dict(os.environ, PYTHONPATH=str(path))
            report = path / "report.json"
            result = subprocess.run([
                sys.executable, str(ROOT / "scripts/shitty/run.py"),
                "--binary", str(BINARY), "--module", "bridge_failure", "--report", str(report),
            ], env=environment, capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 1, result.stderr)
            data = json.loads(report.read_text())
            self.assertEqual(data["failures"], 1)
            self.assertEqual(data["outcomes"][0]["outcome"], "fail")
            self.assertIn("actual", data["outcomes"][0]["detail"])

    def test_unresponsive_adapter_is_terminated_and_reported(self):
        child = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(30)"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, encoding="utf-8")
        with mock.patch("adapter.subprocess.Popen", return_value=child), \
                mock.patch.object(adapter, "REPLY_TIMEOUT", 0.05):
            with self.assertRaisesRegex(RuntimeError, "timed out handling new"):
                adapter.Zetta()
        self.assertIsNotNone(child.poll())

    def test_shitty_only_executable_tests_are_skipped_without_launching_them(self):
        result = unittest.TestResult()
        run.suite_for(["test_embed_example", "test_options"])(result)
        self.assertEqual(len(result.skipped), 2)
        self.assertFalse(result.errors)

    def test_expected_failure_annotations_do_not_hide_zetta_results(self):
        @unittest.expectedFailure
        class Foreign(unittest.TestCase):
            @unittest.expectedFailure
            def test_failure(self):
                self.fail("visible engine failure")

            def test_success(self):
                pass

        suite = unittest.defaultTestLoader.loadTestsFromTestCase(Foreign)
        run.skip_identity_tests(suite)
        result = unittest.TestResult()
        suite(result)
        self.assertEqual(len(result.failures), 1)
        self.assertEqual(len(result.expectedFailures), 0)
        self.assertEqual(len(result.unexpectedSuccesses), 0)

    def test_identity_assertions_skip_and_retained_expected_failures_stay(self):
        class Foreign(unittest.TestCase):
            def test_device_attributes_and_status_reports(self):
                self.fail("Shitty's DA1")

            @unittest.expectedFailure
            def test_configured_quirk(self):
                self.fail("only when configured")

        retained = {f"{Foreign.__module__}.{Foreign.__qualname__}.test_configured_quirk"}
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(Foreign)
        with mock.patch.object(run, "RETAINED_EXPECTED_FAILURES", retained):
            run.skip_identity_tests(suite)
        result = unittest.TestResult()
        suite(result)
        self.assertEqual(len(result.skipped), 1)
        self.assertIn("DA1/DA2 identity", result.skipped[0][1])
        self.assertEqual(len(result.expectedFailures), 1)
        self.assertFalse(result.failures)

    def test_deliberate_differences_are_expected_failures_with_their_reason(self):
        class Foreign(unittest.TestCase):
            def test_policy(self):
                self.fail("Zetta differs on purpose")

        name = f"{Foreign.__module__}.{Foreign.__qualname__}.test_policy"
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(Foreign)
        runner = unittest.TextTestRunner(stream=io.StringIO(), resultclass=run.Result)
        with mock.patch.object(run, "DELIBERATE_DIFFERENCES", {name: "the reason"}):
            run.skip_identity_tests(suite)
            result = runner.run(suite)
        self.assertTrue(result.wasSuccessful())
        self.assertEqual(result.records[0]["outcome"], "expected_failure")
        self.assertEqual(result.records[0]["note"], "the reason")

    def test_recordings_listed_in_xfail_are_expected_to_differ(self):
        path = ROOT / "shitty/tst/xtermjs/t0032-VPB.in"
        self.assertIn(path.stem, run.recorded_quirks())
        outcomes = {}
        for name, replay in [("differs", mock.Mock(side_effect=AssertionError("diff"))),
                             ("matches", mock.Mock())]:
            runner = unittest.TextTestRunner(stream=io.StringIO(), resultclass=run.Result)
            with mock.patch.object(run, "replay_case", replay):
                result = runner.run(run.Recording(path, quirk=True))
            self.assertTrue(result.wasSuccessful())
            outcomes[name] = result.records[0]
        self.assertEqual(outcomes["differs"]["outcome"], "expected_failure")
        self.assertEqual(outcomes["matches"]["outcome"], "pass")
        self.assertIn("xfail.txt", outcomes["matches"]["note"])

    def test_observations_support_dataclass_consumers_without_inventing_fields(self):
        from dataclasses import asdict, fields
        with adapter.Zetta(columns=4, rows=2) as terminal:
            terminal.write(b"A")
            snapshot = terminal.snapshot()
            cell = snapshot.cell(0, 0)
            self.assertEqual(asdict(cell)["char"], "A")
            self.assertIn("foreground", {field.name for field in fields(cell)})
            self.assertNotIn("semantic", asdict(cell))
            self.assertEqual(asdict(snapshot)["lines"][0], "A   ")
            with self.assertRaises(unittest.SkipTest):
                _ = cell.semantic

    def test_large_blank_snapshots_and_long_single_rows_stay_observable(self):
        with adapter.Zetta(columns=16384, rows=1) as terminal:
            snapshot = terminal.snapshot()
            self.assertEqual(len(snapshot.cells), 16384)
            self.assertEqual(snapshot.lines, [" " * 16384])
        with adapter.Zetta(columns=1000, rows=500) as terminal:
            self.assertEqual(len(terminal.snapshot().cells), 500000)

    def test_alternate_screen_wheel_input_is_explicitly_unsupported(self):
        with adapter.Zetta() as terminal:
            terminal.write(b"\x1b[?1049h")
            with self.assertRaisesRegex(unittest.SkipTest, "frontend input"):
                terminal.wheel_up()

    def test_all_skipped_selection_returns_nonzero(self):
        result = subprocess.run([
            sys.executable, str(ROOT / "scripts/shitty/run.py"), "-b", str(BINARY),
            "-m", "test_shell_integration",
        ], capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("OK (skipped=", result.stderr)


if __name__ == "__main__":
    unittest.main()
