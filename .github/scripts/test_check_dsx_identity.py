#!/usr/bin/env python3

from __future__ import annotations

import bz2
import importlib.util
import json
import sys
import tempfile
import unittest
from collections import Counter
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).with_name("check_dsx_identity.py")
SPEC = importlib.util.spec_from_file_location("check_dsx_identity", SCRIPT)
assert SPEC and SPEC.loader
identity = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = identity
SPEC.loader.exec_module(identity)


def legacy_word() -> str:
    return "co" + "dex"


def policy(*allowances: dict) -> dict:
    return {"version": 1, "allowances": list(allowances)}


class CheckDsxIdentityTest(unittest.TestCase):
    def test_baseline_preserves_fingerprints(self) -> None:
        old = identity.Finding("a.txt", "text", "legacy_product", "1" * 20)
        new = identity.Finding("a.txt", "text", "legacy_product", "2" * 20)
        baseline = Counter({old: 1})
        self.assertEqual(sorted((Counter([new]) - baseline).elements()), [new])

    def test_policy_allowance_is_fingerprint_and_count_limited(self) -> None:
        finding = identity.Finding("NOTICE", "text", "legacy_product", "1" * 20)
        allowed = policy(
            {
                "path": finding.path,
                "source": finding.source,
                "term": finding.term,
                "fingerprint": finding.fingerprint,
                "max_count": 1,
                "category": "legal",
                "reason": "Required upstream attribution",
            }
        )
        self.assertEqual(identity.apply_allowances([finding], allowed), [])
        self.assertEqual(identity.apply_allowances([finding, finding], allowed), [finding])

    def test_policy_rejects_stale_allowance(self) -> None:
        allowed = policy(
            {
                "path": "NOTICE",
                "source": "text",
                "term": "legacy_product",
                "fingerprint": "1" * 20,
                "max_count": 1,
                "category": "legal",
                "reason": "Required upstream attribution",
            }
        )
        with self.assertRaisesRegex(ValueError, "stale"):
            identity.apply_allowances([], allowed)

    def test_invalid_baseline_counts_are_rejected(self) -> None:
        base_item = {
            "path": "a.txt",
            "source": "text",
            "term": "legacy_product",
            "fingerprint": "1" * 20,
        }
        for invalid in (-1, 0, True, 1.5, "1"):
            payload = {"version": 2, "findings": [{**base_item, "count": invalid}]}
            data = bz2.compress(json.dumps(payload).encode())
            with self.subTest(invalid=invalid):
                with self.assertRaisesRegex(ValueError, "positive integer"):
                    identity.load_baseline_bytes(data, "test")

    def test_invalid_utf8_does_not_hide_ascii_term(self) -> None:
        word = legacy_word().encode()
        entry = identity.TrackedEntry("notes.txt", "100644", "0" * 40, b"prefix \xff " + word)
        findings = identity.scan_entries([entry], policy())
        self.assertEqual(len(findings), 1)
        self.assertEqual(findings[0].term, "legacy_product")

    def test_gitlink_path_is_scanned_without_blob(self) -> None:
        word = legacy_word()
        entry = identity.TrackedEntry(f"deps/{word}-sdk", "160000", "0" * 40, None)
        findings = identity.scan_entries([entry], policy())
        self.assertEqual(len(findings), 1)
        self.assertEqual(findings[0].source, "path")

    def test_fullwidth_identity_is_normalized(self) -> None:
        entry = identity.TrackedEntry("notes.txt", "100644", "0" * 40, "ＣＯＤＥＸ".encode())
        findings = identity.scan_entries([entry], policy())
        self.assertEqual(len(findings), 1)

    def test_baseline_round_trip_is_compressed_and_deterministic(self) -> None:
        finding = identity.Finding("a.txt", "text", "legacy_product", "1" * 20)
        with tempfile.TemporaryDirectory() as temp_dir:
            baseline_path = Path(temp_dir) / "baseline.json.bz2"
            identity.write_baseline(baseline_path, [finding, finding])
            first = baseline_path.read_bytes()
            identity.write_baseline(baseline_path, [finding, finding])
            second = baseline_path.read_bytes()
            loaded = identity.load_baseline_bytes(second, "test")
        self.assertEqual(first, second)
        self.assertEqual(loaded, Counter({finding: 2}))

    def test_unavailable_base_ref_fails_closed(self) -> None:
        with mock.patch.object(
            identity.subprocess,
            "run",
            return_value=mock.Mock(returncode=1, stdout=b"", stderr=b"missing"),
        ):
            with self.assertRaisesRegex(ValueError, "unavailable"):
                identity.git_show_file("missing", "file", allow_missing=True)

    def test_policy_changes_are_rejected(self) -> None:
        with mock.patch.object(identity, "git_show_file", return_value=b"old"):
            with self.assertRaisesRegex(ValueError, "protected rollout"):
                identity.reject_policy_changes("base", b"new")

    def test_human_path_render_handles_surrogateescape(self) -> None:
        path = b"co" + b"dex-\xff.txt"
        decoded = path.decode("utf-8", errors="surrogateescape")
        self.assertIn("\\xff", identity.display_path(decoded))


if __name__ == "__main__":
    unittest.main()
