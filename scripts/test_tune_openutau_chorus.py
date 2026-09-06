"""Regression tests for OpenUtau tuning parameter normalization."""

import importlib.util
import math
from pathlib import Path
import unittest


MODULE_PATH = Path(__file__).with_name("tune_openutau_chorus.py")
SPEC = importlib.util.spec_from_file_location("tune_openutau_chorus", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class VibratoPeriodTests(unittest.TestCase):
    def test_numeric_period_is_preserved(self):
        self.assertEqual(MODULE.bounded_vibrato_period("80"), 80)

    def test_period_is_bounded(self):
        self.assertEqual(MODULE.bounded_vibrato_period(-1), 40)
        self.assertEqual(MODULE.bounded_vibrato_period(999), 400)

    def test_malformed_and_nonfinite_values_use_default(self):
        self.assertEqual(MODULE.bounded_vibrato_period("bad"), 190)
        self.assertEqual(MODULE.bounded_vibrato_period(math.nan), 190)
        self.assertEqual(MODULE.bounded_vibrato_period(math.inf), 190)

    def test_ustx_tuning_writes_requested_period(self):
        source = (
            "voices: []\n"
            "notes:\n"
            "  - position: 0\n"
            "    tone: 60\n"
            "    lyric: a\n"
            "    vibrato: {length: 80, period: 190, depth: 10, in: 32, out: 28, shift: 0, drift: 0, vol_link: 0}\n"
        )
        tuned = MODULE.tune(source, 8, 10, 28, 0.6, 0.5, 80)
        self.assertIn("period: 80", tuned)


if __name__ == "__main__":
    unittest.main()
