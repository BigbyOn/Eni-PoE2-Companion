"""Dependency vulnerability audit against the OSV database (read-only network).

Usage: python tools/dep_audit.py
Reads Cargo.lock, asks https://api.osv.dev for known advisories of every crate
version, prints a summary. Results are recorded in docs/SECURITY-AUDIT.md.
"""

import json
import pathlib
import sys
import tomllib
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
BATCH = "https://api.osv.dev/v1/querybatch"
VULN = "https://api.osv.dev/v1/vulns/"


def post(url: str, payload: dict) -> dict:
    data = json.dumps(payload).encode()
    req = urllib.request.Request(url, data=data, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as res:
        return json.load(res)


def main() -> int:
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
    packages = [(p["name"], p["version"]) for p in lock["package"] if p.get("source")]
    print(f"packages from crates.io: {len(packages)}")

    queries = [{"package": {"name": n, "version": v, "ecosystem": "crates.io"}} for n, v in packages]
    hits = []
    for i in range(0, len(queries), 1000):
        res = post(BATCH, {"queries": queries[i : i + 1000]})
        for (name, version), r in zip(packages[i : i + 1000], res.get("results", [])):
            ids = [v["id"] for v in r.get("vulns", [])]
            if ids:
                hits.append((name, version, ids))

    print(f"crates with advisories: {len(hits)}")
    for name, version, ids in hits:
        print(f"  {name} {version}: {', '.join(ids)}")
        for vid in ids:
            try:
                with urllib.request.urlopen(VULN + vid, timeout=30) as res:
                    detail = json.load(res)
                summary = detail.get("summary", "")
                sev = detail.get("database_specific", {}).get("severity", "?")
                print(f"    - {vid} [{sev}] {summary}")
            except Exception as err:  # noqa: BLE001
                print(f"    - {vid}: detail unavailable ({err})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
