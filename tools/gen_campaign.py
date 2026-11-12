"""Builds assets/campaign/campaign.json from the factual route + Russian wording.

Usage:  python tools/gen_campaign.py
Input:  tools/route_source.jsonl   (facts: act, zone, tags, level, quest, English goal)
        tools/campaign_ru.py       (Russian goal/quest wording, reward patterns)
Output: assets/campaign/campaign.json
"""

import json
import re
from pathlib import Path

import campaign_ru as ru

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "tools" / "route_source.jsonl"
OUT = ROOT / "assets" / "campaign" / "campaign.json"

BOLD = re.compile(r"\*\*(.+?)\*\*")


def plain(text: str) -> str:
    """Drop the markdown bold markers, keep the words."""
    return BOLD.sub(r"\1", text).strip()


def zone_names(zone: str) -> list[str]:
    """'A -> B' and 'A / B' list two zones; keep the client spelling of each."""
    parts = re.split(r"->|/", zone)
    return [p.strip() for p in parts if p.strip()]


def rewards_for(goal_en: str) -> list[dict]:
    """Permanent rewards are written after '->' in the goal; map them to log patterns."""
    if "->" not in goal_en:
        return []
    tail = goal_en.split("->", 1)[1].lower()
    found = []
    for kind, patterns in ru.REWARDS:
        if any(p.lower() in tail for p in patterns):
            found.append({"kind": kind, "match": patterns})
    return found


def main() -> None:
    rows = [json.loads(line) for line in SRC.read_text(encoding="utf-8").splitlines() if line.strip()]
    if len(rows) != len(ru.RU_GOALS):
        raise SystemExit(f"route has {len(rows)} steps, RU_GOALS has {len(ru.RU_GOALS)}: keep them in sync")

    steps, per_act, seen_zones = [], {}, {}
    for row, goal_ru in zip(rows, ru.RU_GOALS):
        act = row["act"]
        per_act[act] = per_act.get(act, 0) + 1
        names = zone_names(row["zone"])
        key = "|".join(sorted(n.lower() for n in names))
        if key not in seen_zones:
            seen_zones[key] = {
                "id": "z" + re.sub(r"[^a-z0-9]+", "-", names[0].lower()).strip("-"),
                "names": names,
                "act": act,
            }
        zone_id = seen_zones[key]["id"]

        quest_en = (row.get("quest") or "").strip()
        quest = {"en": quest_en, "ru": ru.QUESTS.get(quest_en, quest_en)} if quest_en else None

        tags = list(row["tags"])
        kind = "boss" if "boss" in tags else "town" if "town" in tags else "ascend" if "ascend" in tags else "quest"
        if "tip" in tags and kind == "quest":
            kind = "info"

        steps.append({
            "id": f"a{act}-{per_act[act]:02d}",
            "act": act,
            "zone": zone_id,
            "zones": names,
            "kind": kind,
            "optional": "must" not in tags,
            "level": row.get("level"),
            "goal": {"en": plain(row["en"]), "ru": goal_ru},
            "quest": quest,
            "rewards": rewards_for(row["en"]),
            "tags": tags,
        })

    doc = {
        "schema": 1,
        "version": "2026.11.1",
        "patch": "0.5.5",
        "title": {"ru": "Кампания Path of Exile 2", "en": "Path of Exile 2 campaign"},
        "source": "Порядок зон и цели кампании — сводка сообщества poe2speedrun (MIT), русские формулировки — Eni-PoE2-Companion.",
        "acts": [{"id": aid, "title": {"ru": act["ru"], "en": act["en"]}, "levels": act["levels"]} for aid, act in sorted(ru.ACTS.items())],
        "zones": list(seen_zones.values()),
        "steps": steps,
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(doc, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"{OUT}: {len(steps)} steps, {len(seen_zones)} zones, {len(doc['acts'])} acts")


if __name__ == "__main__":
    main()
