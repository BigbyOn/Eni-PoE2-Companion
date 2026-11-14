"""Regenerate the Russian trade-translation tables.

Run manually when Exiled Exchange 2 updates its localization data:

    python tools/gen_trade_i18n.py

Source: https://github.com/Kvan7/Exiled-Exchange-2 (MIT; see
assets/trade_i18n/NOTICE.txt). Only base/unique names and modifier templates
are kept, nothing is fetched at application runtime.
"""

import json
import pathlib
import urllib.request

BASE = "https://raw.githubusercontent.com/Kvan7/Exiled-Exchange-2/master/renderer/public/data/ru"
OUT = pathlib.Path(__file__).resolve().parent.parent / "assets" / "trade_i18n"


def fetch(name: str) -> str:
    with urllib.request.urlopen(f"{BASE}/{name}", timeout=120) as response:
        return response.read().decode("utf-8")


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)

    items: dict[str, str] = {}
    for line in fetch("items.ndjson").splitlines():
        if not line.strip():
            continue
        entry = json.loads(line)
        ru = (entry.get("name") or "").strip()
        en = (entry.get("refName") or "").strip()
        if ru and en and ru != en:
            items[ru] = en

    stats = []
    for line in fetch("stats.ndjson").splitlines():
        if not line.strip():
            continue
        entry = json.loads(line)
        ref = (entry.get("ref") or "").strip()
        matchers = [(m.get("string") or "").strip() for m in (entry.get("matchers") or [])]
        matchers = [m for m in matchers if m]
        ids = (entry.get("trade") or {}).get("ids") or {}
        explicit = ids.get("explicit") or []
        rune = ids.get("rune") or []
        if ref and matchers and (explicit or rune):
            stats.append({"r": ref, "m": matchers, "id": (explicit or rune)[0]})

    (OUT / "ru_items.json").write_text(
        json.dumps(items, ensure_ascii=False, separators=(",", ":")), encoding="utf-8"
    )
    (OUT / "ru_stats.json").write_text(
        json.dumps(stats, ensure_ascii=False, separators=(",", ":")), encoding="utf-8"
    )
    print(f"items: {len(items)}, stats: {len(stats)}")


if __name__ == "__main__":
    main()
