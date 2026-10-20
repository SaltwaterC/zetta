#!/usr/bin/env python3
"""Run imported terminal conformance tests against Zetta's local terminal core."""

import argparse
import ast
import difflib
import importlib
import functools
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import unittest

import adapter

ROOT = Path(__file__).resolve().parents[2]
TST = ROOT / "shitty" / "tst"
MANIFEST = ROOT / "tools" / "shitty-adapter" / "Cargo.toml"
# These mixed modules import the harness but also directly launch Shitty-only
# executables. They cannot be redirected by replacing the harness class.
FRONTEND_MODULES = {"test_embed_example", "test_options"}
DEFAULT_MODULES = (
    "test_cursor", "test_editing", "test_parser", "test_parser_states",
    "test_cells", "test_dec", "test_modes", "test_tabs", "test_reset_matrix",
    "test_shell_integration", "test_sgr_matrix", "test_osc", "test_protocols",
    "test_alacritty_terminal_core", "test_ghostty_resize",
    "test_xtermjs_buffer_reflow",
)
# Assertions of the exact identity Shitty advertises. Zetta reports the device
# attributes it implements (no sixel, printer or UDKs), so these can only differ.
IDENTITY_TESTS = {
    "test_version_and_tertiary_device_attributes": "Shitty product/version string",
    "test_device_attributes_and_status_reports": "Shitty DA1/DA2 identity",
    "test_s7c1t_and_s8c1t_select_response_encoding": "Shitty DA1 identity",
    "test_decid_is_an_obsolete_alias_for_primary_device_attributes": "Shitty DA1 identity",
    "test_decid_uses_selected_eight_bit_response_controls_in_single_byte_mode":
        "Shitty DA1 identity",
    "test_decscl_selects_response_control_width": "Shitty DA1 identity",
}
# Upstream expected failures that stay expected: they cover a behavior xterm.js
# only shows when configured, rather than a gap Zetta should be held to.
RETAINED_EXPECTED_FAILURES = {
    "test_xtermjs_buffer_reflow.XtermJsBufferReflowTest."
    "test_growth_skips_wrapped_cursor_block_when_configured",
}
# Assertions of a policy Zetta deliberately differs on. They run as expected
# failures, so the difference stays in the report, and a change of behaviour
# shows up as an unexpected success rather than passing unnoticed.
DELIBERATE_DIFFERENCES = {
    "test_osc.OscProtocolTest.test_hyperlink_control_preserves_arbitrary_uri_bytes":
        "invalid UTF-8 in an OSC 8 URI is percent-encoded, because the URI is opened as a string",
    "test_notifications.NotificationProtocolTest."
    "test_capability_reply_declares_only_implemented_payloads":
        "zntfy cannot withdraw a shown notification, so OSC 99 queries do not offer p=close",
}


class Result(unittest.TextTestResult):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.records = []

    def addSuccess(self, test):
        super().addSuccess(test)
        self.records.append(dict(test=test.id(), outcome="pass"))

    def addSkip(self, test, reason):
        super().addSkip(test, reason)
        self.records.append(dict(test=test.id(), outcome="skip", reason=reason))

    def addFailure(self, test, error):
        super().addFailure(test, error)
        self.records.append(dict(test=test.id(), outcome="fail", detail=self.failures[-1][1]))

    def addError(self, test, error):
        super().addError(test, error)
        self.records.append(dict(test=test.id(), outcome="error", detail=self.errors[-1][1]))

    def addExpectedFailure(self, test, error):
        super().addExpectedFailure(test, error)
        self.records.append(dict(test=test.id(), outcome="expected_failure",
                                 detail=self._exc_info_to_string(error, test)))
        if test.id() in DELIBERATE_DIFFERENCES:
            self.records[-1]["note"] = DELIBERATE_DIFFERENCES[test.id()]

    def addUnexpectedSuccess(self, test):
        if getattr(test, "quirk", None):
            # Matching a recorded xterm.js quirk is not a conformance failure.
            self.addSuccess(test)
            self.records[-1]["note"] = "matches the xterm.js quirk listed in xfail.txt"
            return
        super().addUnexpectedSuccess(test)
        self.records.append(dict(test=test.id(), outcome="unexpected_success"))

    def addSubTest(self, test, subtest, error):
        super().addSubTest(test, subtest, error)
        if error is not None:
            outcome = "fail" if issubclass(error[0], test.failureException) else "error"
            self.records.append(dict(test=subtest.id(), outcome=outcome,
                                     detail=self._exc_info_to_string(error, test)))


def replay_case(path):
    """Match the upstream xterm.js adapter's 80x25/default-PTY convention."""
    with adapter.Zetta(columns=80, rows=25) as terminal:
        terminal.write(b"\x1bc\x1b[H")
        terminal.write(path.read_bytes().replace(b"\n", b"\r\n"))
        actual = "\n".join(line.rstrip() for line in terminal.snapshot().lines) + "\n"
    expected = path.with_suffix(".text").read_text(encoding="utf-8")
    normalize = lambda text: "\n".join(line.rstrip() for line in text.split("\n"))
    expected, actual = normalize(expected), normalize(actual)
    if actual != expected:
        difference = "\n".join(difflib.unified_diff(
            expected.splitlines(), actual.splitlines(), fromfile=str(path.with_suffix(".text")),
            tofile="Zetta core", lineterm=""))
        raise AssertionError(difference)


def recorded_quirks():
    """Fixtures listed in xfail.txt record xterm.js behavior contrary to the standards."""
    path = TST / "xtermjs" / "xfail.txt"
    lines = path.read_text(encoding="utf-8").splitlines() if path.is_file() else ()
    return {line.strip() for line in lines if line.strip() and not line.startswith("#")}


class Recording(unittest.TestCase):
    def __init__(self, path, quirk=False):
        super().__init__()
        self.path = path
        self.quirk = quirk
        if quirk:
            self.__unittest_expecting_failure__ = True

    def id(self):
        return f"xtermjs/{self.path.stem}"

    def __str__(self):
        return self.id()

    def runTest(self):
        replay_case(self.path)


class UnsupportedModule(unittest.TestCase):
    def __init__(self, name):
        super().__init__()
        self.name = name

    def id(self):
        return self.name

    def runTest(self):
        raise unittest.SkipTest("module tests Shitty's build/frontend rather than the terminal harness")


def suite_for(modules):
    suite = unittest.TestSuite()
    loader = unittest.TestLoader()
    for name in modules:
        path = TST / (name.split(".", 1)[0] + ".py")
        if name.split(".", 1)[0] in FRONTEND_MODULES:
            suite.addTest(UnsupportedModule(name))
            continue
        if path.is_file():
            tree = ast.parse(path.read_text(encoding="utf-8"))
            imports_harness = any(
                isinstance(node, ast.ImportFrom) and node.module == "harness"
                and any(alias.name == "Shitty" for alias in node.names)
                for node in ast.walk(tree))
            if not imports_harness:
                suite.addTest(UnsupportedModule(name))
                continue
        tests = loader.loadTestsFromName(name)
        skip_identity_tests(tests)
        suite.addTests(tests)
    return suite


def skip_identity_tests(suite):
    for test in suite:
        if isinstance(test, unittest.TestSuite):
            skip_identity_tests(test)
        elif test.id() in RETAINED_EXPECTED_FAILURES:
            continue
        elif test.id() in DELIBERATE_DIFFERENCES:
            test.__unittest_expecting_failure__ = True
        elif (getattr(test, "__unittest_expecting_failure__", False) or
              getattr(getattr(test, test._testMethodName), "__unittest_expecting_failure__", False)):
            test.__unittest_expecting_failure__ = False
            original = getattr(test, test._testMethodName)
            @functools.wraps(original)
            def ordinary_test(original=original):
                return original()
            ordinary_test.__unittest_expecting_failure__ = False
            setattr(test, test._testMethodName, ordinary_test)
        elif test._testMethodName in IDENTITY_TESTS:
            def skip(reason=IDENTITY_TESTS[test._testMethodName]):
                raise unittest.SkipTest(f"{reason} assertion")
            setattr(test, test._testMethodName, skip)


def main():
    parser = argparse.ArgumentParser(description=__doc__, epilog=(
        "Examples: python3 scripts/shitty/run.py; "
        "python3 scripts/shitty/run.py -m test_cursor -m test_editing; "
        "python3 scripts/shitty/run.py -a -r artifacts/shitty.json. "
        "Requires Python 3.10+ and the pinned Rust toolchain; no display or Shitty build."
    ))
    parser.add_argument("-m", "--module", action="append", help=(
        "upstream unittest module or dotted test name, repeatable; defaults to shell-relevant "
        "cursor/edit/parser/resize/protocol modules plus xterm.js recordings"))
    parser.add_argument("-a", "--all", action="store_true", help="discover every upstream Python test module")
    parser.add_argument("-b", "--binary", type=Path, help="use an already built adapter executable")
    parser.add_argument("-r", "--report", type=Path, help="write outcomes, skips, and failure details as JSON")
    parser.add_argument("-v", "--verbose", action="store_true", help="show each test result")
    parser.add_argument("-t", "--timeout", type=float, default=10,
                        help="seconds to wait for each adapter reply (default: 10)")
    parser.add_argument("-l", "--list", action="store_true", help="list selectable upstream modules and recordings")
    args = parser.parse_args()
    if not 0 < args.timeout < float("inf"):
        parser.error("--timeout must be positive and finite")
    adapter.REPLY_TIMEOUT = args.timeout
    if args.all and args.module:
        parser.error("--all and --module are mutually exclusive")
    if not (TST / "harness.py").is_file():
        parser.error("missing submodule; run git submodule update --init shitty")
    available = sorted(path.stem for path in TST.glob("test_*.py"))
    fixtures = sorted((TST / "xtermjs").glob("*.in"))
    if args.list:
        print("\n".join(available + [f"xtermjs/{path.stem}" for path in fixtures]))
        return 0
    binary = args.binary
    if binary is None:
        target = ROOT / "target" / "shitty-adapter"
        subprocess.run([
            os.environ.get("CARGO", "cargo"), "build", "--locked",
            "--manifest-path", str(MANIFEST), "--target-dir", str(target),
        ], cwd=ROOT, check=True)
        binary = target / "debug" / ("zetta-shitty-adapter.exe" if os.name == "nt" else "zetta-shitty-adapter")
    binary = binary.resolve()
    if not binary.is_file():
        parser.error(f"adapter executable does not exist: {binary}")
    sys.path.insert(0, str(TST))
    harness = importlib.import_module("harness")
    adapter.install(harness, binary)
    modules = available if args.all else args.module or DEFAULT_MODULES
    # Recordings are directly selectable and always included in the default/full runs.
    selected = [name for name in modules if not name.startswith("xtermjs/")]
    suite = suite_for(selected)
    replay_names = {name.split("/", 1)[1] for name in modules if name.startswith("xtermjs/")}
    missing = replay_names - {path.stem for path in fixtures}
    if missing:
        parser.error("unknown recording: " + ", ".join(sorted(missing)))
    quirks = recorded_quirks()
    for path in fixtures:
        if args.all or args.module is None or path.stem in replay_names:
            suite.addTest(Recording(path, path.stem in quirks))
    started = time.monotonic()
    result = unittest.TextTestRunner(
        verbosity=2 if args.verbose else 1, resultclass=Result,
    ).run(suite)
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        revision = subprocess.check_output(
            ["git", "-C", str(TST.parent), "rev-parse", "HEAD"], text=True).strip()
        report = dict(schema_version=1, backend="zetta-terminal-core", submodule_revision=revision,
                      tests_run=result.testsRun, duration_seconds=time.monotonic() - started,
                      failures=len(result.failures), errors=len(result.errors),
                      skipped=len(result.skipped), expected_failures=len(result.expectedFailures),
                      unexpected_successes=len(result.unexpectedSuccesses), outcomes=result.records)
        args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    # An entirely skipped selection is not a successful conformance run.
    exercised = any(record["outcome"] != "skip" for record in result.records)
    return 0 if result.wasSuccessful() and exercised else 1


if __name__ == "__main__":
    sys.exit(main())
