// The in-game panel: compact / standard / extended. It never takes focus, so
// clicks anywhere else go to the game. Rust keeps the window at exactly the
// size this page renders.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (sel) => document.querySelector(sel);

let S = null;
let CAMP = null;
let BUILD = null;
let preview = null; // live look preview from the settings sliders

const esc = (v) =>
  String(v ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const profile = () => S?.saved.profiles.find((p) => p.id === S.saved.active) || S?.saved.profiles[0] || null;
const zoneLabel = (step) => l10n(step?.zone_label) || step?.zones?.[0] || "";
const norm = (z) => String(z || "").trim().toLowerCase().split(/\s+/).join(" ").replace(/^the /, "");
const step = () => (CAMP ? CAMP.steps[profile()?.step ?? 0] : null);
const mode = () => preview?.mode || S?.saved.overlay.mode || "standard";

function shownIndexes() {
  if (!CAMP) return [];
  return CAMP.steps.map((s, i) => i).filter((i) => S.saved.route !== "must" || !CAMP.steps[i].optional);
}

function progress() {
  const shown = shownIndexes();
  const cur = profile()?.step ?? 0;
  let pos = shown.findIndex((i) => i >= cur);
  if (pos < 0) pos = shown.length - 1;
  return { pos: pos + 1, total: shown.length };
}

function nextDestination(index) {
  const here = CAMP?.steps[index];
  if (!here) return null;
  const shown = shownIndexes();
  const pos = shown.findIndex((i) => i >= index);
  for (const i of shown.slice(pos + 1)) {
    const s = CAMP.steps[i];
    if (!s.zones.some((z) => here.zones.some((h) => norm(h) === norm(z)))) return s;
  }
  return null;
}

// **bold** in the route data marks names, as in the original overlay.
function goalHtml(text) {
  return esc(text).replace(/\*\*(.+?)\*\*/g, '<span class="name">$1</span>');
}

function renderRegex() {
  const panel = $("#panel");
  const look = preview || S.saved.overlay;
  panel.hidden = false;
  panel.className = "regex";
  panel.style.setProperty("--panel", `rgba(12,11,9,${(look.opacity ?? 88) / 100})`);
  panel.style.zoom = `${(look.scale ?? 100) / 100}`;
  $("#act").textContent = t("overlay.regex_title");
  $("#lvls").textContent = "";
  const list = S.regexes || [];
  $("#body").innerHTML = list.length
    ? `<div class="small" style="margin-bottom:5px">${esc(t("overlay.regex_hint"))}</div>` +
      list
        .map(
          (r, i) =>
            `<button class="regex-item" data-regex="${esc(r.value)}" title="${esc(r.value)}"><b>${i + 1}. ${esc(r.name || "Regex")}</b><span>${esc(r.value)}</span></button>`
        )
        .join("")
    : `<div class="small">${esc(t("overlay.regex_empty"))}</div>`;
  reportSize();
}

function itemToast() {
  const li = S.last_item;
  if (!li) return "";
  const age = Date.now() - new Date(li.ts).getTime();
  if (!(age >= 0 && age < 8000)) return "";
  setTimeout(() => render(), 1500);
  return `<div class="item-toast">${esc(t("overlay.item_imported"))}: <b>${esc(li.name)}</b> · ${li.score}</div>`;
}

function render() {
  if (!S || !CAMP) return;
  if (S.regex_open) {
    renderRegex();
    return;
  }
  const p = profile();
  const s = step();
  const panel = $("#panel");
  const m = mode();
  const look = preview || S.saved.overlay;
  document.documentElement.lang = S.saved.lang;
  panel.className = `${m === "compact" ? "mini" : ""} ${look.opacity < 50 ? "glass" : ""}`;
  panel.style.setProperty("--panel", `rgba(12,11,9,${(look.opacity ?? 88) / 100})`);
  panel.style.zoom = `${(look.scale ?? 100) / 100}`;
  panel.hidden = false;

  const build = BUILD;
  const stage = build?.stages.find((x) => p && p.level >= x.levels[0] && p.level <= x.levels[1]) || build?.stages.at(-1);

  if (!s) {
    $("#act").textContent = "Eni-PoE2-Companion";
    $("#lvls").textContent = "";
    $("#body").innerHTML = `<div class="goal">${esc(t("home.no_step"))}</div>`;
    reportSize();
    return;
  }

  const { pos, total } = progress();
  const here = p && s.zones.some((z) => norm(z) === norm(p.zone));
  const dest = nextDestination(profile()?.step ?? 0);
  const go = !here && p?.zone;
  $("#act").textContent = `${t("home.act")} ${p?.act ?? s.act} · ${pos}/${total}`;
  $("#lvls").textContent = p ? `${t("chip.level").replace(":", "")} ${p.level}` : "";
  $("#lvls").classList.toggle("warn", Boolean(p && S.area_level && Math.abs(S.area_level - p.level) > 3 + Math.floor(p.level / 16)));
  $("#bar").title = t("overlay.drag_hint");
  document.querySelector('[data-act="prev"]').title = t("overlay.prev");
  document.querySelector('[data-act="next"]').title = t("overlay.next");
  document.querySelector('[data-act="mode"]').title = t("overlay.mode");
  document.querySelector('[data-act="hide"]').title = t("overlay.hide");

  const toast = itemToast();
  if (m === "compact") {
    const zone = esc(p?.zone || zoneLabel(s));
    $("#body").innerHTML = go
      ? `<div class="go">${esc(t("overlay.go_to"))} <span class="dest">${esc(zoneLabel(s))}</span>:</div> <span class="goal">${goalHtml(l10n(s.goal))}</span>`
      : `${toast}<span class="zone">${zone}${S.zone_unknown ? " · " + esc(t("overlay.zone_unknown")) : ""}</span> <span class="goal">${goalHtml(l10n(s.goal))}</span>
         ${dest ? `<span class="then">${esc(t("overlay.next_dest"))}: <b>${esc(zoneLabel(dest))}</b></span>` : ""}`;
    reportSize();
    return;
  }

  let html = toast;
  if (go) html += `<div class="go">${esc(t("overlay.go_to"))} <span class="dest">${esc(zoneLabel(s))}</span> <span class="pct">· ${esc(p.zone)}</span></div>`;
  html += `<div class="zone">${esc(zoneLabel(s))}${S.zone_unknown ? ` · <span class="warn">${esc(t("overlay.zone_unknown"))}</span>` : ""}</div>`;
  html += `<div class="goal">${goalHtml(l10n(s.goal))}</div>`;
  if (s.quest) html += `<div class="quest">${esc(t("home.quest"))}: ${esc(l10n(s.quest))}</div>`;
  if (dest) html += `<div class="then">${esc(t("overlay.next_dest"))}: <b>${esc(zoneLabel(dest))}</b></div>`;
  html += `<div class="meta"><span class="pct">${esc(t("overlay.progress"))} ${pos}/${total}</span><span class="bar"><i style="width:${total ? Math.round((pos / total) * 100) : 0}%"></i></span></div>`;

  if (m === "extended") {
    if (p) {
      html += `<div class="char"><b>${esc(p.name || "—")}</b><span>${esc(p.class)}${p.ascendancy ? " · " + esc(p.ascendancy) : ""}</span><span class="pct">${esc(t("chip.level"))} ${p.level}</span></div>`;
    }
    if (build && stage) {
      const done = new Set(p?.build_done || []);
      const nextItems = stage.checklist.filter((c) => !done.has(c.id)).slice(0, 3);
      html += `<div class="build">${esc(t("overlay.build"))}: <b>${esc(l10n(build.name))}</b> · ${esc(l10n(stage.title))}</div>`;
      if (nextItems.length) html += `<ul class="checks">${nextItems.map((c) => `<li>${esc(l10n(c.label))}</li>`).join("")}</ul>`;
    }
    const rewards = (p?.rewards || []).slice(-2).reverse();
    if (rewards.length) html += `<ul class="rewards">${rewards.map((r) => `<li>${esc(r.text)}</li>`).join("")}</ul>`;
    if (!S.log_found) html += `<div class="hint warn">${esc(t("home.log_missing"))}</div>`;
  }
  $("#body").innerHTML = html;
  reportSize();
}

function reportSize() {
  const panel = $("#panel");
  if (panel.hidden) return;
  // CSS zoom is already part of the border box, so this is the window size.
  const r = panel.getBoundingClientRect();
  invoke("panel_size", { w: r.width, h: r.height });
}

new ResizeObserver(reportSize).observe($("#panel"));

document.addEventListener("click", (e) => {
  const rx = e.target.closest("[data-regex]");
  if (rx) {
    invoke("copy_regex", { value: rx.dataset.regex });
    return;
  }
  const btn = e.target.closest("[data-act]");
  if (btn) {
    const what = btn.dataset.act;
    if (what === "prev") invoke("campaign_step", { delta: -1 });
    else if (what === "next") invoke("campaign_step", { delta: 1 });
    else if (what === "hide") invoke("overlay_toggle");
    else if (what === "mode") {
      const order = ["compact", "standard", "extended"];
      const next = order[(order.indexOf(mode()) + 1) % order.length];
      invoke("save_overlay", { overlay: { ...S.saved.overlay, mode: next } });
    }
    return;
  }
  // A click on the compact line opens the standard panel.
  if (mode() === "compact" && !e.target.closest("#bar")) {
    invoke("save_overlay", { overlay: { ...S.saved.overlay, mode: "standard" } });
  }
});

// Drag by the header (compact: the whole line). Rust moves the window from the
// real cursor position, so the panel does not chase its own coordinates.
let dragStart = null;
$("#panel").addEventListener("pointerdown", (e) => {
  if (e.button !== 0 || e.target.closest("button")) return;
  if (mode() !== "compact" && !e.target.closest("#bar")) return;
  dragStart = { x: e.screenX, y: e.screenY, moved: false };
  invoke("drag", { phase: "start" });
  $("#panel").setPointerCapture(e.pointerId);
});
$("#panel").addEventListener("pointermove", (e) => {
  if (!dragStart) return;
  if (Math.abs(e.screenX - dragStart.x) + Math.abs(e.screenY - dragStart.y) > 3) dragStart.moved = true;
  if (dragStart.moved) invoke("drag", { phase: "move" });
});
$("#panel").addEventListener("pointerup", () => {
  if (!dragStart) return;
  const moved = dragStart.moved;
  dragStart = null;
  invoke("drag", { phase: "end" });
  if (!moved && mode() === "compact") invoke("save_overlay", { overlay: { ...S.saved.overlay, mode: "standard" } });
});

async function boot() {
  const boot = await invoke("bootstrap");
  S = boot.state;
  CAMP = boot.campaign;
  BUILD = boot.builds.find((b) => b.id === profile()?.build_id) || boot.builds[0] || null;
  setLang(S.saved.lang);
  render();
  await listen("state", (e) => {
    S = e.payload;
    if (S.saved.lang !== LANG) setLang(S.saved.lang);
    preview = null;
    render();
  });
  await listen("look", (e) => {
    preview = { ...(preview || {}), ...e.payload };
    render();
  });
}

boot().catch((err) => {
  document.body.innerHTML = `<pre style="color:#e8685a;padding:10px;font:12px monospace">${esc(err?.stack || err)}</pre>`;
});
