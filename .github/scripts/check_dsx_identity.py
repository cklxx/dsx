#!/usr/bin/env python3

"""Track legacy product identity references during the DSX migration."""

from __future__ import annotations

import argparse
import bz2
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unicodedata
from collections import Counter
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_BASELINE = ROOT / ".github" / "dsx-identity-baseline.json.bz2"
DEFAULT_POLICY = ROOT / ".github" / "dsx-identity-policy.json"
BASELINE_REPO_PATH = ".github/dsx-identity-baseline.json.bz2"
POLICY_REPO_PATH = ".github/dsx-identity-policy.json"
BASELINE_VERSION = 2
TERMS = {
    "legacy_product": "co" + "dex",
    "upstream_company": "open" + "ai",
    "upstream_account": "chat" + "gpt",
}


@dataclass(frozen=True, order=True)
class Finding:
    path: str
    source: str
    term: str
    fingerprint: str


@dataclass(frozen=True)
class TrackedEntry:
    path: str
    mode: str
    oid: str
    data: bytes | None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-ref")
    parser.add_argument("--json", action="store_true", dest="json_output")
    parser.add_argument("--strict", action="store_true")
    parser.add_argument("--update-baseline", action="store_true")
    args = parser.parse_args()

    entries = tracked_entries(ROOT)
    blobs = {entry.path: entry.data for entry in entries}
    policy_data = blobs.get(POLICY_REPO_PATH)
    if policy_data is None:
        raise ValueError(f"missing staged policy: {POLICY_REPO_PATH}")
    policy = load_policy_bytes(policy_data, POLICY_REPO_PATH)
    if args.base_ref:
        reject_policy_changes(args.base_ref, policy_data)
    findings = scan_entries(entries, policy)

    if args.update_baseline:
        write_baseline(DEFAULT_BASELINE, findings)
        print(f"Updated {BASELINE_REPO_PATH} with {len(findings)} findings.")
        return 0

    if args.strict:
        regressions = findings
        baseline_count = 0
    else:
        baseline_data = blobs.get(BASELINE_REPO_PATH)
        if baseline_data is None:
            raise ValueError(f"missing staged baseline: {BASELINE_REPO_PATH}")
        baseline = load_baseline_bytes(baseline_data, BASELINE_REPO_PATH)
        baseline_count = sum(baseline.values())
        regressions = sorted((Counter(findings) - baseline).elements())
        if args.base_ref:
            reject_baseline_widening(args.base_ref, baseline)

    report = build_report(findings, regressions, baseline_count, args.strict)
    if args.json_output:
        print(json.dumps(report, indent=2, sort_keys=True))
    else:
        print_human_report(report)
    return 1 if regressions else 0


def load_policy_bytes(data: bytes, source: str) -> dict:
    value = json.loads(data.decode("utf-8"))
    if set(value) != {"version", "allowances"} or value["version"] != 1:
        raise ValueError(f"unsupported policy schema in {source}")
    allowances = value["allowances"]
    if not isinstance(allowances, list):
        raise ValueError("policy allowances must be a list")
    for allowance in allowances:
        if not isinstance(allowance, dict) or set(allowance) != {
            "path",
            "source",
            "term",
            "fingerprint",
            "max_count",
            "category",
            "reason",
        }:
            raise ValueError("invalid policy allowance")
        if allowance["category"] not in {
            "legal",
            "history",
            "migration",
            "third_party_immutable",
        }:
            raise ValueError(f"invalid allowance category: {allowance['category']}")
        if not isinstance(allowance["reason"], str) or not allowance["reason"].strip():
            raise ValueError("allowance reason must not be empty")
        validate_finding_fields(allowance)
        validate_count(allowance["max_count"], "allowance max_count")
    return value


def scan_entries(entries: list[TrackedEntry], policy: dict) -> list[Finding]:
    findings: list[Finding] = []
    for entry in entries:
        normalized_path = normalize(entry.path)
        for term_name, needle in TERMS.items():
            count = normalized_path.count(needle)
            findings.extend(
                Finding(entry.path, "path", term_name, fingerprint(normalized_path))
                for _ in range(count)
            )

        if entry.data is None or entry.path in {BASELINE_REPO_PATH, POLICY_REPO_PATH}:
            continue
        for raw_line in entry.data.splitlines():
            normalized_line = normalize_bytes(raw_line)
            line_fingerprint = fingerprint_bytes(raw_line.strip())
            for term_name, needle in TERMS.items():
                count = normalized_line.count(needle)
                findings.extend(
                    Finding(entry.path, "text", term_name, line_fingerprint)
                    for _ in range(count)
                )

    return apply_allowances(sorted(findings), policy)


def apply_allowances(findings: list[Finding], policy: dict) -> list[Finding]:
    allowances: Counter[Finding] = Counter()
    for item in policy["allowances"]:
        finding = Finding(item["path"], item["source"], item["term"], item["fingerprint"])
        if finding in allowances:
            raise ValueError("duplicate policy allowance")
        allowances[finding] = item["max_count"]

    residual = []
    for finding in findings:
        if allowances[finding] > 0:
            allowances[finding] -= 1
        else:
            residual.append(finding)
    stale = [finding for finding, count in allowances.items() if count]
    if stale:
        raise ValueError(f"stale policy allowance: {stale[0].path}")
    return residual


def tracked_entries(root: Path) -> list[TrackedEntry]:
    env = {**os.environ, "GIT_NO_REPLACE_OBJECTS": "1"}
    result = subprocess.run(
        ["git", "-C", str(root), "ls-files", "--stage", "-z"],
        check=True,
        capture_output=True,
        env=env,
    )
    entries: list[tuple[str, str, str]] = []
    for raw_entry in result.stdout.split(b"\0"):
        if not raw_entry:
            continue
        metadata, raw_path = raw_entry.split(b"\t", 1)
        mode, oid, stage = metadata.decode("ascii").split()
        if stage != "0":
            raise ValueError("identity check requires an index without merge conflicts")
        path = raw_path.decode("utf-8", errors="surrogateescape")
        entries.append((path, mode, oid))

    blob_entries = [(path, mode, oid) for path, mode, oid in entries if mode != "160000"]
    process = subprocess.Popen(
        ["git", "-C", str(root), "cat-file", "--batch"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        env=env,
    )
    assert process.stdin is not None and process.stdout is not None
    blobs: dict[str, bytes] = {}
    try:
        for path, _, oid in blob_entries:
            process.stdin.write(oid.encode("ascii") + b"\n")
            process.stdin.flush()
            header = process.stdout.readline().rstrip(b"\n").split()
            if len(header) != 3 or header[1] != b"blob":
                raise ValueError(f"failed to read indexed blob for {display_path(path)}")
            size = int(header[2])
            data = process.stdout.read(size)
            if len(data) != size or process.stdout.read(1) != b"\n":
                raise ValueError(f"truncated indexed blob for {display_path(path)}")
            blobs[path] = data
    finally:
        process.stdin.close()
        process.stdout.close()
        return_code = process.wait()
        if return_code:
            raise subprocess.CalledProcessError(return_code, process.args)

    return sorted(
        (
            TrackedEntry(path, mode, oid, None if mode == "160000" else blobs[path])
            for path, mode, oid in entries
        ),
        key=lambda entry: entry.path.encode("utf-8", errors="surrogateescape"),
    )


def normalize(value: str) -> str:
    return unicodedata.normalize("NFKC", value).casefold()


def normalize_bytes(value: bytes) -> str:
    return normalize(value.decode("utf-8", errors="surrogateescape"))


def fingerprint(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8", errors="surrogateescape")).hexdigest()[:20]


def fingerprint_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()[:20]


def write_baseline(path: Path, findings: list[Finding]) -> None:
    payload = {
        "version": BASELINE_VERSION,
        "findings": encode_counter(Counter(findings)),
    }
    compressed = bz2.compress(
        (json.dumps(payload, separators=(",", ":"), sort_keys=True) + "\n").encode("utf-8"),
        compresslevel=9,
    )
    atomic_write(path, compressed)


def atomic_write(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temp_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as temp_file:
            temp_file.write(data)
            temp_file.flush()
            os.fsync(temp_file.fileno())
        os.replace(temp_name, path)
    except BaseException:
        Path(temp_name).unlink(missing_ok=True)
        raise


def load_baseline_bytes(data: bytes, source: str) -> Counter[Finding]:
    try:
        value = json.loads(bz2.decompress(data).decode("utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ValueError(f"invalid baseline in {source}: {exc}") from exc
    if set(value) != {"version", "findings"} or value["version"] != BASELINE_VERSION:
        raise ValueError(f"unsupported baseline schema in {source}")

    counter: Counter[Finding] = Counter()
    for item in value["findings"]:
        if not isinstance(item, dict) or set(item) != {
            "path",
            "source",
            "term",
            "fingerprint",
            "count",
        }:
            raise ValueError("invalid baseline finding")
        validate_finding_fields(item)
        validate_count(item["count"], "baseline count")
        finding = Finding(item["path"], item["source"], item["term"], item["fingerprint"])
        if finding in counter:
            raise ValueError("duplicate baseline finding")
        counter[finding] = item["count"]
    return counter


def validate_finding_fields(item: dict) -> None:
    if not isinstance(item["path"], str) or not item["path"]:
        raise ValueError("finding path must be a non-empty string")
    if item["source"] not in {"path", "text"}:
        raise ValueError("finding source must be path or text")
    if item["term"] not in TERMS:
        raise ValueError("finding term is unknown")
    if not isinstance(item["fingerprint"], str) or len(item["fingerprint"]) != 20:
        raise ValueError("finding fingerprint must be a 20-character string")


def validate_count(value: object, label: str) -> None:
    if type(value) is not int or value <= 0:
        raise ValueError(f"{label} must be a positive integer")


def encode_counter(counter: Counter[Finding]) -> list[dict]:
    return [
        {
            "path": finding.path,
            "source": finding.source,
            "term": finding.term,
            "fingerprint": finding.fingerprint,
            "count": count,
        }
        for finding, count in sorted(counter.items())
    ]


def reject_baseline_widening(base_ref: str, current: Counter[Finding]) -> None:
    base_data = git_show_file(base_ref, BASELINE_REPO_PATH, allow_missing=True)
    if base_data is None:
        return
    base = load_baseline_bytes(base_data, f"{base_ref}:{BASELINE_REPO_PATH}")
    if current - base:
        raise ValueError("identity baseline may only shrink")


def reject_policy_changes(base_ref: str, current_policy_data: bytes) -> None:
    base_data = git_show_file(base_ref, POLICY_REPO_PATH, allow_missing=True)
    if base_data is not None and current_policy_data != base_data:
        raise ValueError("identity policy changes require a separate protected rollout")


def git_show_file(base_ref: str, path: str, *, allow_missing: bool) -> bytes | None:
    env = {**os.environ, "GIT_NO_REPLACE_OBJECTS": "1"}
    verify = subprocess.run(
        ["git", "-C", str(ROOT), "cat-file", "-e", f"{base_ref}^{{commit}}"],
        capture_output=True,
        env=env,
    )
    if verify.returncode:
        raise ValueError(f"base ref is unavailable: {base_ref}")
    result = subprocess.run(
        ["git", "-C", str(ROOT), "show", f"{base_ref}:{path}"],
        capture_output=True,
        env=env,
    )
    if not result.returncode:
        return result.stdout
    exists = subprocess.run(
        ["git", "-C", str(ROOT), "cat-file", "-e", f"{base_ref}:{path}"],
        capture_output=True,
        env=env,
    )
    if allow_missing and exists.returncode:
        return None
    raise ValueError(f"failed to read {path} from base ref {base_ref}")


def build_report(findings: list[Finding], regressions: list[Finding], baseline_count: int, strict: bool) -> dict:
    return {
        "mode": "strict" if strict else "baseline",
        "baselineCount": baseline_count,
        "findingCount": len(findings),
        "countsByTerm": dict(sorted(Counter(f.term for f in findings).items())),
        "regressionCount": len(regressions),
        "regressionsByTerm": dict(sorted(Counter(f.term for f in regressions).items())),
        "regressions": encode_counter(Counter(regressions)),
    }


def display_path(path: str) -> str:
    return path.encode("utf-8", errors="surrogateescape").decode("utf-8", errors="backslashreplace")


def print_human_report(report: dict) -> None:
    print(
        f"DSX identity check ({report['mode']}): {report['findingCount']} tracked findings; "
        f"{report['regressionCount']} regressions."
    )
    if report["countsByTerm"]:
        print("Current debt: " + ", ".join(f"{k}={v}" for k, v in report["countsByTerm"].items()))
    for finding in report["regressions"]:
        print(
            f"  {display_path(finding['path'])}: new {finding['source']} "
            f"{finding['term']} reference x{finding['count']}"
        )
    if report["regressionCount"]:
        print("Remove the new reference, then refresh and review the migration baseline.")


if __name__ == "__main__":
    sys.exit(main())
