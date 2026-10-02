"""Decodes and re-encodes the `perseid samples` of every model of the torture and edge fixtures.

Each fixture is generated as its own SDK, next to the samples of its models. Every sample of
every model must decode into the generated type and encode back to the same JSON, up to the
documented normalizations of `differences`: date-times compare as instants and decimal strings
as numbers. Integers (int64 included) compare exactly.
"""

import json
import re
import unittest
from decimal import Decimal

from _support import fixtures, generate_fixture, import_package, instant, perseid

DECIMAL = re.compile(r"-?\d+(?:\.\d+)?")


def differences(want: object, got: object, path: str = "$") -> list[str]:
    """Where `got` differs from `want`, as JSON, beyond the documented normalizations."""
    mismatch = [f"{path}: expected {want!r}, got {got!r}"]
    if isinstance(want, bool) or isinstance(got, bool):
        return [] if want is got else mismatch
    if isinstance(want, (int, float)) and isinstance(got, (int, float)):
        # Python compares an int and a float exactly, so a rounded int64 is a difference.
        return [] if want == got else mismatch
    if isinstance(want, str) and isinstance(got, str):
        if want == got:
            return []
        instants = instant(want), instant(got)
        if instants[0] is not None and instants[0] == instants[1]:
            return []
        if DECIMAL.fullmatch(want) and DECIMAL.fullmatch(got) and Decimal(want) == Decimal(got):
            return []
        return mismatch
    if isinstance(want, dict) and isinstance(got, dict):
        out: list[str] = []
        for key in sorted(set(want) | set(got)):
            if key not in got:
                out.append(f"{path}.{key}: missing, expected {want[key]!r}")
            elif key not in want:
                out.append(f"{path}.{key}: unexpected {got[key]!r}")
            else:
                out += differences(want[key], got[key], f"{path}.{key}")
        return out
    if isinstance(want, list) and isinstance(got, list):
        if len(want) != len(got):
            return [f"{path}: expected {len(want)} items, got {len(got)}"]
        out = []
        for index, (a, b) in enumerate(zip(want, got)):
            out += differences(a, b, f"{path}[{index}]")
        return out
    return [] if want is None and got is None else mismatch


class NormalizationTest(unittest.TestCase):
    def test_only_documented_normalizations_are_accepted(self) -> None:
        self.assertEqual(differences("2024-01-02T03:04:05Z", "2024-01-02T04:04:05.000+01:00"), [])
        self.assertEqual(differences("12.50", "12.5"), [])
        self.assertEqual(differences(1, 1.0), [])
        self.assertTrue(differences(9007199254740993, 9007199254740992.0))
        self.assertTrue(differences(9007199254740993, 9007199254740992))
        self.assertTrue(differences(True, 1))
        self.assertTrue(differences(None, 0))
        self.assertTrue(differences("a", "b"))
        self.assertTrue(differences({"a": None}, {}))
        self.assertTrue(differences({}, {"a": None}))
        self.assertTrue(differences([1, 2], [1]))


class SampleRoundTripTest(unittest.TestCase):
    def test_every_sample_of_every_model_round_trips(self) -> None:
        specs = [fixtures() / "torture.yaml", *sorted(fixtures().glob("edge-*.yaml"))]
        total = 0
        for spec in specs:
            with self.subTest(spec=spec.name):
                total += self.check_spec(spec.name)
        self.assertGreater(total, 100, "the fixtures hold many models")

    def check_spec(self, fixture: str) -> int:
        stem = fixture.removesuffix(".yaml")
        project = generate_fixture(fixture, "Rt" + stem.title().replace("-", ""))
        perseid(project, "samples", "--out", "samples.json")
        samples = json.loads((project / "samples.json").read_text())
        if not samples:
            return 0
        package = import_package(project)
        count = 0
        for name, entry in sorted(samples.items()):
            self.assertTrue(entry["samples"], f"{fixture} {name} has no samples")
            annotation = getattr(package.models, entry["names"]["python"])
            for sample in entry["samples"]:
                with self.subTest(spec=fixture, model=name, sample=sample["name"]):
                    self.round_trip(package, annotation, sample["json"])
                    count += 1
        return count

    def round_trip(self, package: object, annotation: object, data: object) -> None:
        serialization = package.serialization  # type: ignore[attr-defined]
        parsed = serialization.from_json_value(annotation, data)
        encoded = json.loads(json.dumps(serialization.to_json_value(parsed, annotation)))
        self.assertEqual(differences(data, encoded), [], f"re-encoded as {encoded!r}")
        # Decoding what was encoded gives the same value again.
        self.assertEqual(serialization.from_json_value(annotation, encoded), parsed)


if __name__ == "__main__":
    unittest.main()
