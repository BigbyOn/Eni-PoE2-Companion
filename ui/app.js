// Eni-PoE2-Companion — main window. Plain JS, no build step.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];
const esc = (v) =>
  String(v ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

let S = null; // snapshot
let CAMP = null; // campaign route
let BUILDS = [];
let VERSION = "";
let SHOTS = []; // screenshots of the active profile
let ITEMS = []; // imported items
let HISTORY = []; // loaded history records
let section = "home";
let capturing = null; // hotkey button waiting for keys
let activeShot = null; // screenshot in the modal
let historyLoaded = false;
const HFILTER = { kind: "", profile: "", query: "" };
const THUMBS = new Map();
const CLASSES = ["Ranger", "Huntress", "Witch", "Sorceress", "Warrior", "Monk", "Mercenary", "Druid"];
const ASCENDANCIES = [
  "Deadeye", "Pathfinder", "Amazon", "Ritualist", "Infernalist", "Blood Mage", "Stormweaver",
  "Chronomancer", "Titan", "Warbringer", "Invoker", "Acolyte of Chayula", "Witchhunter", "Gemling Legionnaire",
];
const SECTIONS = ["home", "character", "build", "campaign", "trade", "history", "shots", "settings"];
const SHOT_KINDS = ["item", "inventory", "passive", "gems", "other"];
const REASONING = ["auto", "low", "medium", "high"];

const providerInfo = (id) => (S?.ai_providers || []).find((p) => p.id === id) || null;
const providerLabel = (id) => providerInfo(id)?.label || id;
const aiModel = () => S?.saved.ai.model || providerInfo(S?.saved.ai.provider)?.default_model || "";
const aiReady = () => Boolean(S?.saved.ai.enabled && S?.saved.ai.provider && S.saved.ai.provider !== "disabled" && S.ai_has_key);
// Same rule as the Rust side: only OpenAI vision ids get the picture.
function aiVision(provider = S?.saved.ai.provider, model = aiModel()) {
  const m = String(model || "").toLowerCase();
  return provider === "openai" && ["gpt-4o", "gpt-4.1", "gpt-5", "o3", "o4", "chatgpt-4o"].some((p) => m.startsWith(p));
}

// ---------------------------------------------------------------- helpers
const profile = () => S?.saved.profiles.find((p) => p.id === S.saved.active) || S?.saved.profiles[0] || null;
const stepAt = (i) => (CAMP ? CAMP.steps[i] : null);
const activeBuild = () => BUILDS.find((b) => b.id === profile()?.build_id) || BUILDS[0] || null;
const buildStage = (b, level) =>
  b?.stages.find((s) => level >= s.levels[0] && level <= s.levels[1]) || (b?.stages.length ? b.stages[b.stages.length - 1] : null);
const norm = (z) => String(z || "").trim().toLowerCase().split(/\s+/).join(" ").replace(/^the /, "");
const fmtTime = (iso) => {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return esc(iso);
  return d.toLocaleString(LANG === "en" ? "en-GB" : "ru-RU", { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" });
};
const kindLabel = (k) => t(KIND_KEY[k] || k);
const classDatalists = (value) => {
  const list = [...new Set([...CLASSES, value].filter(Boolean))];
  return `<datalist id="classes">${list.map((c) => `<option value="${esc(c)}">`).join("")}</datalist>
          <datalist id="ascendancies">${ASCENDANCIES.map((a) => `<option value="${esc(a)}">`).join("")}</datalist>`;
};

function toast(message, isError = false) {
  const el = $("#toast");
  el.textContent = message;
  el.className = isError ? "error" : "";
  el.hidden = false;
  clearTimeout(toast._t);
  toast._t = setTimeout(() => (el.hidden = true), isError ? 6000 : 2800);
}

function stepStatus(index) {
  const p = profile();
  if (!p) return "ahead";
  if (index < p.step) return "done";
  if (index === p.step) return "current";
  if (p.done?.includes(stepAt(index)?.id)) return "done";
  return "ahead";
}

function routeIndexes() {
  if (!CAMP) return [];
  return CAMP.steps.map((s, i) => ({ s, i })).filter(({ s }) => S.saved.route !== "must" || !s.optional);
}

function progressInfo() {
  const p = profile();
  const shown = routeIndexes();
  const pos = shown.findIndex(({ i }) => i >= (p?.step ?? 0));
  return { pos: (pos < 0 ? shown.length - 1 : pos) + 1, total: shown.length };
}

function stepZoneName(step) {
  if (!step) return "";
  return l10n(step.zone_label) || step?.zones?.[0] || "";
}

// Russian label plus the client's English spelling, so the zone is unambiguous.
function stepZoneFull(step) {
  if (!step) return "";
  const label = step.zone_label;
  const ru = l10n(label);
  const en = label?.en || "";
  return LANG === "ru" && en && ru !== en ? `${ru} (${en})` : ru;
}

function sameZone(step, zone) {
  return (step?.zones || []).some((z) => norm(z) === norm(zone));
}

function nextDestination(index) {
  const here = stepAt(index);
  if (!here) return null;
  const shown = routeIndexes();
  const pos = shown.findIndex(({ i }) => i >= index);
  for (const { s } of shown.slice(pos + 1)) {
    if (!s.zones.some((z) => here.zones.some((h) => norm(h) === norm(z)))) return s;
  }
  return null;
}

function actTitle(actId) {
  return l10n(CAMP?.acts.find((a) => a.id === actId)?.title) || `${t("home.act")} ${actId}`;
}

function go(id) {
  if (!SECTIONS.includes(id)) id = "home";
  section = id;
  location.hash = id;
  renderNav();
  renderSection();
  $("#content").scrollTop = 0;
}

function renderNav() {
  $("#nav-items").innerHTML = SECTIONS.map(
    (id) => `<button data-nav="${id}" class="${id === section ? "active" : ""}"><span class="dot"></span>${esc(t("nav." + id))}</button>`
  ).join("");
}

function renderChips() {
  const p = profile();
  const chips = [];
  chips.push(
    `<span class="chip ${S.game_running ? "on" : "off"}">${esc(t("chip.game"))}: <b>${esc(S.game_running ? t("chip.on") : t("chip.off"))}</b></span>`
  );
  chips.push(
    `<span class="chip ${S.log_found ? "on" : "off"}" title="${esc(S.log_path || "")}">${esc(t("chip.log"))}: <b>${esc(
      S.log_found ? t("chip.log_ok") : t("chip.log_none")
    )}</b></span>`
  );
  if (p) {
    chips.push(`<span class="chip">${esc(t("chip.level"))}: <b>${p.level}</b></span>`);
    chips.push(`<span class="chip">${esc(t("chip.act"))}: <b>${p.act}</b></span>`);
  }
  chips.push(`<span class="chip">${esc(t("chip.offline"))}</span>`);
  if (S.hotkey_error) chips.push(`<span class="chip warn" title="${esc(S.hotkey_error)}">${esc(t("set.hotkeys"))}: <b>!</b></span>`);
  $("#chips").innerHTML = chips.join("");
}

function renderSection() {
  $("#page-title").textContent = t("nav." + section);
  $("#ver").textContent = `v${VERSION}`;
  const render = { home: renderHome, character: renderChar, build: renderBuild, campaign: renderCampaign, trade: renderTrade, history: renderHistory, shots: renderShots, settings: renderSettings }[section];
  try {
    render();
  } catch (err) {
    $("#content").innerHTML = `<div class="card"><h2>${esc(t("common.error"))}</h2><pre class="diag">${esc(err?.stack || err)}</pre></div>`;
  }
  updateSessionClocks();
  $("#content").focus({ preventScroll: true });
}

// ---------------------------------------------------------------- home
function zoneCard() {
  const todos = S.zone_todos || [];
  const pending = todos.filter((t) => !t.done);
  const done = todos.filter((t) => t.done);
  const zone = S.zone_progress?.zone_ru || S.saved.profiles?.[0]?.zone || "";
  const row = (item, isDone) => `<div class="check-item ${isDone ? "done" : ""}">
      <input type="checkbox" data-act="zone-todo-toggle" data-id="${esc(item.id)}" ${isDone ? "checked" : ""}>
      <span>${esc(item.text)}</span>
      <button class="icon" data-act="zone-todo-remove" data-id="${esc(item.id)}" title="${esc(t("zone.remove"))}">✕</button>
    </div>`;
  return `<div class="card">
    <div class="row between">
      <h2>${esc(t("zone.title"))}</h2>
      <span class="small">${esc(zone)}</span>
    </div>
    <div class="small" style="margin:4px 0 8px">${esc(t("zone.pending"))}: <b>${pending.length}</b>${done.length ? ` · ${esc(t("zone.done_count"))}: ${done.length}` : ""}</div>
    <div>${pending.length ? pending.map((item) => row(item, false)).join("") : `<div class="small">${esc(t("zone.none"))}</div>`}</div>
    ${done.length ? `<details style="margin-top:6px"><summary class="small">${esc(t("zone.done_count"))}: ${done.length}</summary>${done.map((item) => row(item, true)).join("")}</details>` : ""}
    <div class="row" style="margin-top:8px">
      <input id="zt-input" style="flex:1;min-width:200px" placeholder="${esc(t("zone.placeholder"))}">
      <button class="btn sm primary" data-act="zone-todo-add">${esc(t("zone.add"))}</button>
    </div>
  </div>`;
}

function renderHome() {
  const p = profile();
  const step = stepAt(p?.step ?? 0);
  const { pos, total } = progressInfo();
  const pct = total ? Math.round((pos / total) * 100) : 0;
  const dest = p ? nextDestination(p.step) : null;
  const here = p && step && sameZone(step, p.zone);
  const zoneLine = p?.zone ? esc(p.zone) : `<span class="small">${esc(t("common.unknown"))}</span>`;

  const warnings = [];
  if (!S.game_running)
    warnings.push(`<div class="card tight warn-text">${esc(t("home.game_missing"))}</div>`);
  if (!S.log_found) warnings.push(`<div class="card tight warn-text">${esc(t("home.log_missing"))}</div>`);

  const cards = [
    { label: t("chip.game"), value: S.game_running ? t("chip.on") : t("chip.off"), good: S.game_running },
    { label: t("chip.log"), value: S.log_found ? S.log_encoding || t("chip.log_ok") : t("chip.log_none"), good: S.log_found },
    { label: t("home.character"), value: p ? `${p.name || "—"} · ${p.class} · ${p.level}` : "—" },
    { label: t("chip.act"), value: p ? actTitle(p.act) : "—" },
  ];

  const rewards = (p?.rewards || []).slice(-3).reverse();

  $("#content").innerHTML = `
    ${warnings.join("")}
    ${zoneCard()}
    <div class="card">
      <div class="row between">
        <h2>${esc(t("home.next"))}</h2>
        <span class="small">${esc(actTitle(p?.act || 1))} · ${esc(t("home.progress"))} ${pos}/${total}</span>
      </div>
      <div class="bar" style="margin:8px 0 12px"><i style="width:${pct}%"></i></div>
      ${
        step
          ? `<div class="row between" style="align-items:flex-start">
               <div style="min-width:0">
                 <div class="small">${esc(t("home.zone"))}</div>
                 <div class="zone" style="font-variant:small-caps;letter-spacing:.04em;color:var(--gold);font-size:16px">${zoneLine}
                   ${S.zone_unknown ? `<span class="tag">${esc(t("home.zone_unknown"))}</span>` : ""}</div>
                 <div style="font:20px/1.35 var(--serif);color:var(--gold2);margin:6px 0 4px">${esc(l10n(step.goal))}</div>
                 ${step.quest ? `<div class="small">${esc(t("home.quest"))}: ${esc(l10n(step.quest))}</div>` : ""}
                 ${step.level ? `<div class="small">${esc(t("char.level"))}: ${esc(step.level)}</div>` : ""}
                 ${!here && dest ? `<div class="small" style="margin-top:6px">${esc(t("home.go"))}: <b style="color:var(--gold)">${esc(stepZoneFull(step))}</b></div>` : ""}
                 ${here && dest ? `<div class="small" style="margin-top:6px">${esc(t("overlay.next_dest"))}: <b style="color:var(--gold)">${esc(stepZoneFull(dest))}</b></div>` : ""}
               </div>
             </div>
             <div class="row" style="margin-top:12px">
               <button class="btn primary" data-act="campaign-done">${esc(t("home.done"))}</button>
               <button class="btn" data-act="campaign-prev">◀ ${esc(t("home.prev"))}</button>
               <button class="btn" data-act="campaign-next">${esc(t("home.next_btn"))} ▶</button>
               <button class="btn ghost" data-act="go-campaign">${esc(t("nav.campaign"))}</button>
             </div>`
          : `<div class="small">${esc(t("home.no_step"))}</div>`
      }
    </div>
    <div class="grid c4">${cards
      .map(
        (c) => `<div class="card tight">
          <div class="small">${esc(c.label)}</div>
          <div style="font-size:15px;color:${c.good === false ? "var(--bad)" : c.good === true ? "var(--good)" : "var(--ink)"}">${esc(c.value)}</div>
        </div>`
      )
      .join("")}</div>
    <div class="card tight">
      <div class="row">
        <span class="chip">${esc(t("session.title"))}: <b id="sess-elapsed">—</b></span>
        <span class="chip">${esc(t("session.zone"))}: <b id="sess-zone">—</b></span>
        <span class="chip">${esc(t("session.deaths"))}: <b>${S.session_deaths || 0}</b></span>
        <span class="chip">${esc(t("session.levels"))}: <b>+${S.session_levels || 0}</b></span>
        <span class="chip">${esc(t("session.zones"))}: <b>${S.session_zones || 0}</b></span>
      </div>
    </div>
    <div class="card">
      <h3>${esc(t("home.quick"))}</h3>
      <div class="row">
        <button class="btn" data-act="overlay-toggle">${esc(t("home.overlay"))}: ${esc(S.saved.overlay.visible ? t("home.overlay_hide") : t("home.overlay_show"))}</button>
        <button class="btn" data-act="shot-take">${esc(t("home.shot"))}</button>
        <button class="btn ghost" data-act="log-auto">${esc(t("home.relog"))}</button>
        <button class="btn ghost" data-act="go-character">${esc(t("nav.character"))}</button>
        <button class="btn ghost" data-act="go-build">${esc(t("nav.build"))}</button>
      </div>
    </div>
    <div class="grid c2">
      <div class="card">
        <h3>${esc(t("home.recent"))}</h3>
        ${
          HISTORY.length
            ? `<ul class="list">${HISTORY.slice(-8).reverse().map((e) => `<li><span class="mono small nowrap">${fmtTime(e.ts)}</span><span>${esc(e.title)}${e.zone ? ` <span class="small">· ${esc(e.zone)}</span>` : ""}</span></li>`).join("")}</ul>`
            : `<div class="small">${esc(t("home.no_events"))}</div>`
        }
      </div>
      <div class="card">
        <h3>${esc(t("char.rewards"))}</h3>
        ${
          rewards.length
            ? `<ul class="list">${rewards.map((r) => `<li><span class="mono small nowrap">${fmtTime(r.ts)}</span><span>${esc(r.text)}</span></li>`).join("")}</ul>`
            : `<div class="small">${esc(t("char.no_rewards"))}</div>`
        }
        <div class="hr"></div>
        <h3>${esc(t("home.ai"))}</h3>
        <div class="small">${
          aiReady()
            ? esc(t("ai.enabled_line", { provider: providerLabel(S.saved.ai.provider), model: aiModel() })) +
              ` · ${esc(aiVision() ? t("ai.vision_yes") : t("ai.vision_no"))}`
            : esc(t("home.ai_off"))
        }</div>
      </div>
    </div>`;
  if (!historyLoaded) ensureHistory().then(() => { if (section === "home") renderSection(); });
}

// ---------------------------------------------------------------- character
function renderChar() {
  const p = profile();
  if (!p) return;
  const tasks = p.done?.length || 0;
  $("#content").innerHTML = `
    <div class="card">
      <div class="row between">
        <h2>${esc(t("char.profiles"))}</h2>
        <div class="row">
          <button class="btn sm" data-act="profile-new">+ ${esc(t("char.new"))}</button>
          <button class="btn sm danger" data-act="profile-delete" ${S.saved.profiles.length <= 1 ? "disabled" : ""}>${esc(t("char.delete"))}</button>
        </div>
      </div>
      <div class="row" style="margin-top:6px">${S.saved.profiles
        .map(
          (pr) =>
            `<button class="btn ${pr.id === p.id ? "primary" : ""}" data-act="profile-select" data-id="${esc(pr.id)}">${esc(pr.name || t("common.unknown"))} <span class="small" style="opacity:.7">${pr.level}</span></button>`
        )
        .join("")}</div>
    </div>
    <div class="card">
      <h2>${esc(p.name || t("common.unknown"))} ${p.name_auto ? `<span class="tag">${esc(t("char.name_auto"))}</span>` : `<span class="tag">${esc(t("char.name_manual"))}</span>`}</h2>
      <div class="grid c2">
        <div class="field"><label>${esc(t("char.name"))}</label>
          <input type="text" id="pf-name" value="${esc(p.name)}" maxlength="48"></div>
        <div class="field"><label>${esc(t("char.class"))}</label>
          <input type="text" id="pf-class" list="classes" value="${esc(p.class)}">${classDatalists(p.class)}</div>
        <div class="field"><label>${esc(t("char.ascendancy"))}</label>
          <input type="text" id="pf-asc" list="ascendancies" value="${esc(p.ascendancy)}"></div>
        <div class="field"><label>${esc(t("char.level"))}</label>
          <input type="number" id="pf-level" min="1" max="100" value="${p.level}"></div>
        <div class="field"><label>${esc(t("char.zone"))}</label>
          <input type="text" id="pf-zone" list="zones" value="${esc(p.zone)}">
          <datalist id="zones">${(CAMP?.zones || []).map((z) => `<option value="${esc(z.names[0])}">`).join("")}</datalist>
        </div>
        <div class="field"><label>${esc(t("char.build"))}</label>
          <select id="pf-build">${BUILDS.map((b) => `<option value="${esc(b.id)}" ${b.id === p.build_id ? "selected" : ""}>${esc(l10n(b.name))}</option>`).join("")}</select></div>
      </div>
      <div class="row" style="margin-top:12px">
        <button class="btn primary" data-act="profile-save">${esc(t("char.save"))}</button>
        <button class="btn ghost" data-act="campaign-done">${esc(t("camp.mark_done"))}</button>
        <span class="small">${esc(t("char.zone_from_log"))}</span>
      </div>
      <div class="small" style="margin-top:10px">${esc(t("char.limits"))}</div>
    </div>
    <div class="grid c2">
      <div class="card">
        <h3>${esc(t("char.rewards"))}</h3>
        ${(p.rewards || []).length
          ? `<ul class="list">${[...p.rewards].reverse().slice(0, 12).map((r) => `<li><span class="mono small nowrap">${fmtTime(r.ts)}</span><span>${esc(r.text)}</span></li>`).join("")}</ul>`
          : `<div class="small">${esc(t("char.no_rewards"))}</div>`}
      </div>
      <div class="card">
        <h3>${esc(t("char.levels"))}</h3>
        <dl class="kv">
          <dt>${esc(t("char.level"))}</dt><dd>${p.level}</dd>
          <dt>${esc(t("chip.act"))}</dt><dd>${esc(actTitle(p.act))}</dd>
          <dt>${esc(t("char.tasks"))}</dt><dd>${tasks}</dd>
          <dt>${esc(t("char.zone"))}</dt><dd>${esc(p.zone || "—")}</dd>
        </dl>
        <div class="hr"></div>
        ${(p.levels || []).length
          ? `<ul class="list">${[...p.levels].reverse().slice(0, 10).map((l) => `<li><span class="mono small nowrap">${fmtTime(l.ts)}</span><span>${esc(t("char.level"))} ${l.level}</span></li>`).join("")}</ul>`
          : `<div class="small">—</div>`}
      </div>
    </div>`;
}

async function saveProfile() {
  const p = profile();
  if (!p) return;
  await invoke("profile_save", {
    patch: {
      id: p.id,
      name: $("#pf-name")?.value ?? p.name,
      class: $("#pf-class")?.value ?? p.class,
      ascendancy: $("#pf-asc")?.value ?? p.ascendancy,
      level: Number($("#pf-level")?.value || p.level),
      zone: $("#pf-zone")?.value ?? p.zone,
      build_id: $("#pf-build")?.value ?? p.build_id,
    },
  });
  toast(t("char.saved"));
}

// ---------------------------------------------------------------- build
function renderBuild() {
  const b = activeBuild();
  if (!b) {
    $("#content").innerHTML = `<div class="card"><div class="small">${esc(t("build.no_build"))}</div></div>`;
    return;
  }
  const p = profile();
  const level = p?.level || 1;
  const stage = buildStage(b, level);
  const done = new Set(p?.build_done || []);
  const total = b.stages.reduce((n, s) => n + s.checklist.length, 0);
  const doneCount = [...done].filter((id) => b.stages.some((s) => s.checklist.some((c) => c.id === id))).length;

  $("#content").innerHTML = `
    <div class="card">
      <div class="row between">
        <h2>${esc(t("items.title"))}</h2>
        <div class="row">
          <button class="btn primary" data-act="item-import">${esc(t("items.import"))}</button>
        </div>
      </div>
      <div class="small" style="margin:4px 0 8px">${esc(t("items.hint"))}</div>
      <div id="item-list"></div>
    </div>
    <div class="card">
      <div class="row between">
        <div>
          <h2>${esc(l10n(b.name))}</h2>
          <div class="small">${esc(b.class)} · ${esc(b.ascendancy)} · ${esc(t("build.patch"))} ${esc(b.patch)}</div>
        </div>
        <div class="row" style="align-items:flex-end">
          <div class="field" style="min-width:220px">
            <label>${esc(t("build.select"))}</label>
            <select id="build-select">${BUILDS.map((x) => `<option value="${esc(x.id)}" ${x.id === b.id ? "selected" : ""}>${esc(l10n(x.name))}</option>`).join("")}</select>
          </div>
          <button class="btn sm ghost" data-act="pob-import">${esc(t("build.import_pob"))}</button>
          <button class="btn sm ghost" data-act="guide-import">${esc(t("build.import_url"))}</button>
        </div>
      </div>
      <p class="muted" style="margin:8px 0 0">${esc(l10n(b.summary))}</p>
      <div class="row" style="margin-top:10px">
        <span class="small">${esc(t("build.progress"))}: ${doneCount}/${total}</span>
        <div class="bar" style="flex:1;min-width:140px"><i style="width:${total ? Math.round((doneCount / total) * 100) : 0}%"></i></div>
      </div>
    </div>
    <div class="card">
      <div class="row between">
        <h2>${esc(t("build.stage"))}: ${esc(l10n(stage.title))}</h2>
        <span class="small">${esc(t("build.for_level"))} ${level}</span>
      </div>
      <p class="muted" style="margin:6px 0 12px">${esc(l10n(stage.focus))}</p>
      <h3>${esc(t("build.skills"))}</h3>
      <ul class="list">${stage.skills
        .map(
          (s) => `<li><span><b style="color:var(--ink)">${esc(s.name)}</b>
            ${s.supports?.length ? `<span class="chips" style="margin-left:6px">${s.supports.map((x) => `<span class="gem">${esc(x)}</span>`).join("")}</span>` : ""}
            ${s.note ? `<div class="small">${esc(l10n(s.note))}</div>` : ""}</span></li>`
        )
        .join("")}</ul>
      ${stage.passives?.length ? `<h3 style="margin-top:12px">${esc(t("build.passives"))}</h3><ul class="list">${stage.passives.map((x) => `<li>${esc(l10n(x))}</li>`).join("")}</ul>` : ""}
      ${stage.gear?.length ? `<h3 style="margin-top:12px">${esc(t("build.gear"))}</h3><ul class="list">${stage.gear.map((x) => {
            const line = l10n(x);
            const inside = line.match(/\(([^)]+)\)\s*$/);
            const base = (inside ? inside[1] : line).trim();
            return `<li>${esc(line)} <button class="link" data-act="find-item" data-q="${esc(base)}">${esc(t("trade.find"))}</button></li>`;
          }).join("")}</ul>` : ""}
      ${stage.checklist?.length ? `<h3 style="margin-top:12px">${esc(t("build.checklist"))}</h3><div>${stage.checklist
        .map((c) => `<label class="check-item ${done.has(c.id) ? "done" : ""}"><input type="checkbox" data-item="${esc(c.id)}" ${done.has(c.id) ? "checked" : ""}><span>${esc(l10n(c.label))}</span></label>`)
        .join("")}</div>` : ""}
    </div>
    <div class="grid c2">
      ${[
        ["build.stats", b.priorities?.stats],
        ["build.weapon", b.priorities?.weapon],
        ["build.armour", b.priorities?.armour],
        ["build.accessories", b.priorities?.accessories],
      ]
        .filter(([, list]) => list?.length)
        .map(([key, list]) => `<div class="card"><h3>${esc(t(key))}</h3><ul class="list">${list.map((x) => `<li>${esc(l10n(x))}</li>`).join("")}</ul></div>`)
        .join("")}
    </div>
    ${b.tips?.length ? `<div class="card"><h3>${esc(t("build.tips"))}</h3><ul class="list">${b.tips.map((x) => `<li>${esc(l10n(x))}</li>`).join("")}</ul></div>` : ""}
    <div class="card">
      <h3>${esc(t("build.sources"))}</h3>
      <ul class="list">${(b.sources || []).map((s) => `<li><button class="link" data-act="link" data-url="${esc(s.url)}">${esc(s.label)}</button></li>`).join("")}</ul>
      ${b.disclaimer ? `<div class="hr"></div><p class="small">${esc(l10n(b.disclaimer))}</p>` : ""}
    </div>`;

  loadItems();
  loadGuide(b.id);
}

// ---------------------------------------------------------------- campaign
function renderCampaign() {
  const p = profile();
  const step = p ? stepAt(p.step) : null;
  const dest = p ? nextDestination(p.step) : null;
  const { pos, total } = progressInfo();

  const zoneOptions = (CAMP?.zones || [])
    .slice()
    .sort((a, b) => a.act - b.act || a.names[0].localeCompare(b.names[0]))
    .map((z) => `<option value="${esc(z.names[0])}">${esc(z.names[0])}</option>`)
    .join("");

  let html = `
    <div class="card">
      <div class="row between">
        <div>
          <h2>${esc(actTitle(p?.act || 1))}</h2>
          <div class="small">${esc(t("home.progress"))}: ${pos} ${esc(t("shot.of"))} ${total} ${esc(t("camp.steps"))} · ${esc(t("camp.route"))}: ${esc(t(S.saved.route === "must" ? "camp.route_must" : "camp.route_all"))}</div>
        </div>
        <div class="row">
          <button class="btn sm" data-act="route-toggle">${esc(t(S.saved.route === "must" ? "camp.route_all" : "camp.route_must"))}</button>
          <button class="btn sm" data-act="campaign-prev">◀</button>
          <button class="btn sm" data-act="campaign-next">▶</button>
        </div>
      </div>
      <div class="bar" style="margin:10px 0 12px"><i style="width:${total ? Math.round((pos / total) * 100) : 0}%"></i></div>
      ${step ? `<div class="small">${esc(t("camp.next"))}: <b style="color:var(--gold)">${esc(dest ? stepZoneFull(dest) : "—")}</b></div>` : ""}
    </div>
    <div class="card">
      <h3>${esc(t("camp.manual_zone"))}</h3>
      <div class="row">
        <select id="manual-zone" style="max-width:320px"><option value="">${esc(t("camp.choose_zone"))}</option>${zoneOptions}</select>
        <button class="btn" data-act="manual-zone">${esc(t("camp.manual_zone"))}</button>
      </div>
      <div class="small" style="margin-top:6px">${esc(t("camp.manual_zone_hint"))}</div>
      ${S.zone_unknown ? `<div class="small warn-text" style="margin-top:6px">${esc(t("home.zone_unknown"))}: ${esc(profile()?.zone || "")}</div>` : ""}
      <div class="small" style="margin-top:6px">${esc(t("camp.not_detected"))}</div>
    </div>
    ${S.zone_unknown && profile()?.zone
      ? `<div class="card warn-border">
           <h3>${esc(t("camp.map_title"))}: ${esc(profile().zone)}</h3>
           <div class="small" style="margin:4px 0 8px">${esc(t("camp.map_hint", { zone: profile().zone }))}</div>
           <div class="row">
             <select id="map-zone" style="max-width:320px"><option value="">${esc(t("camp.choose_zone"))}</option>${zoneOptions}</select>
             <button class="btn primary" data-act="zone-map">${esc(t("camp.map_btn"))}</button>
             ${S.area_id ? `<span class="small mono">${esc(t("camp.area"))}: ${esc(S.area_id)}</span>` : ""}
           </div>
         </div>`
      : ""}`;

  const shown = routeIndexes();
  for (const act of CAMP?.acts || []) {
    const actSteps = shown.filter(({ i }) => stepAt(i)?.act === act.id);
    if (!actSteps.length) continue;
    const doneInAct = actSteps.filter(({ i }) => stepStatus(i) === "done").length;
    html += `<div class="act-head"><h3>${esc(l10n(act.title))}</h3>
      <span class="small">${doneInAct}/${actSteps.length} · ${esc(t("char.level"))} ${act.levels[0]}–${act.levels[1]}</span></div>`;
    for (const { s, i } of actSteps) {
      const status = stepStatus(i);
      const rewards = (s.rewards || []).map((r) => `<span class="tag gem">${esc(r.kind)}</span>`).join("");
      html += `<div class="step ${status}">
        <div class="mark">${status === "done" ? "✓" : i + 1}</div>
        <div>
          <div class="goal">${esc(l10n(s.goal))}</div>
          <div class="meta">
            <span class="zone">${esc(stepZoneFull(s))}</span>
            ${s.optional ? `<span class="tag">${esc(t("camp.optional"))}</span>` : ""}
            ${(s.tags || []).filter((x) => ["boss", "town", "ascend", "gem"].includes(x)).map((x) => `<span class="tag ${esc(x)}">${esc(x)}</span>`).join("")}
            ${rewards}
            ${s.level ? `<span>${esc(t("char.level"))}: ${esc(s.level)}</span>` : ""}
            ${s.quest ? `<span>· ${esc(l10n(s.quest))}</span>` : ""}
            ${status === "current" ? `<span class="tag must">${esc(t("camp.current"))}</span>` : ""}
            ${status === "done" ? `<span class="tag">${esc(t("camp.done"))}</span>` : ""}
          </div>
        </div>
        <div class="actions">
          ${status === "current" ? `<button class="btn sm primary" data-act="campaign-done">${esc(t("home.done"))}</button>` : ""}
          <button class="btn sm ghost" data-act="campaign-goto" data-step="${i}">${esc(t("camp.goto"))}</button>
        </div>
      </div>`;
    }
  }
  $("#content").innerHTML = html;
}

// ---------------------------------------------------------------- history
function renderTrade() {
  const enabled = S.saved.trade?.enabled;
  $("#content").innerHTML = `
    <div class="card">
      <div class="row between">
        <h2>${esc(t("trade.title"))}</h2>
        <span class="small">${esc(t("trade.league"))}: ${esc(S.saved.trade?.league || t("trade.auto"))}</span>
      </div>
      <div class="small" style="margin:4px 0 10px">${esc(t("trade.hint"))}</div>
      ${S.trade_request ? `<div class="card tight" style="margin-bottom:10px">
        <div class="small">${esc(t("trade.incoming"))}: <b>${esc(S.trade_request.buyer)}</b></div>
        <div><b>${esc(S.trade_request.item)}</b></div>
        <div class="small">${esc(S.trade_request.price)}${S.trade_request.league ? " · " + esc(S.trade_request.league) : ""}${S.trade_request.stash ? " · " + esc(S.trade_request.stash) : ""}</div>
        <div class="row" style="margin-top:6px">
          <button class="btn sm primary" data-act="trade-invite">${esc(t("trade.invite"))}</button>
          <button class="btn sm ghost" data-act="trade-trade">${esc(t("trade.trade"))}</button>
          <button class="btn sm ghost" data-act="trade-reply">${esc(t("trade.reply"))}</button>
          <button class="btn sm danger" data-act="trade-clear">${esc(t("trade.clear"))}</button>
        </div>
      </div>` : ""}
      ${enabled ? `<div class="card tight" style="margin-bottom:10px"><div class="row between">
        <span class="small" id="rate-line">${esc(t("trade.rate_wait"))}</span>
        <button class="btn sm ghost" data-act="rate-refresh">${esc(t("trade.refresh"))}</button>
      </div></div>` : ""}
      ${
        enabled
          ? `<div class="row"><input type="text" id="trade-q" style="flex:1;min-width:240px" placeholder="${esc(t("trade.placeholder"))}">
             <button class="btn primary" data-act="trade-run">${esc(t("trade.search"))}</button></div>
             <div class="row" style="margin-top:8px">
               <input type="number" id="trade-max" min="0" step="1" placeholder="${esc(t("trade.max"))}" style="width:130px">
               <select id="trade-cur">
                 <option value="exalted">exalted</option>
                 <option value="divine">divine</option>
                 <option value="chaos">chaos</option>
               </select>
               <select id="trade-cat">
                 <option value="">${esc(t("trade.cat_any"))}</option>
                 <option value="weapon">${esc(t("trade.cat_weapon"))}</option>
                 <option value="armour">${esc(t("trade.cat_armour"))}</option>
                 <option value="accessory">${esc(t("trade.cat_accessory"))}</option>
                 <option value="flask">${esc(t("trade.cat_flask"))}</option>
                 <option value="jewel">${esc(t("trade.cat_jewel"))}</option>
                 <option value="gem">${esc(t("trade.cat_gem"))}</option>
               </select>
               <label class="small"><input type="checkbox" id="trade-online" checked> ${esc(t("trade.online"))}</label>
             </div>
             <div id="trade-recent" class="chips" style="margin-top:8px"></div>
        <div id="trade-out" style="margin-top:10px"></div>`
          : `<div class="small bad-text">${esc(t("trade.disabled"))}</div>`
      }
    </div>`;

  if (enabled) loadRate();
  renderRecent();
}

async function loadRate() {
  const line = $("#rate-line");
  if (!line) return;
  line.textContent = t("trade.rate_wait");
  try {
    const r = await invoke("trade_rate");
    const money = (n) => (Number.isInteger(n) ? String(n) : n.toFixed(1));
    const parts = [esc(t("trade.rate_line").replace("{n}", money(r.exalted_per_divine)))];
    if (r.league) parts.push(esc(r.league));
    if (r.chaos_per_divine) parts.push(`1 Divine ≈ ${money(r.chaos_per_divine)} Chaos`);
    if (r.offers > 0) parts.push(`${r.offers} ${esc(t("trade.rate_offers"))}`);
    if (r.source) parts.push(esc(r.source));
    line.innerHTML = parts.join(" · ");
  } catch (e) {
    line.textContent = `${t("trade.rate_none")}: ${String(e)}`;
  }
}

const RECENT_KEY = "eni-trade-recent";

function recentSearches() {
  try {
    const value = JSON.parse(localStorage.getItem(RECENT_KEY) || "[]");
    return Array.isArray(value) ? value.filter((x) => typeof x === "string") : [];
  } catch {
    return [];
  }
}

function rememberSearch(query) {
  const list = recentSearches().filter((x) => x !== query);
  list.unshift(query);
  localStorage.setItem(RECENT_KEY, JSON.stringify(list.slice(0, 8)));
}

function renderRecent() {
  const box = $("#trade-recent");
  if (!box) return;
  const list = recentSearches();
  box.innerHTML = list.length
    ? `<span class="small">${esc(t("trade.recent"))}:</span> ` +
      list.map((q) => `<button class="tag" data-act="trade-recent" data-q="${esc(q)}">${esc(q)}</button>`).join(" ") +
      ` <button class="link" data-act="trade-recent-clear">${esc(t("trade.recent_clear"))}</button>`
    : "";
}

async function runTradeSearch(q) {
  const out = $("#trade-out");
  if (!out) return;
  const query = (q || "").trim();
  if (query.length < 2) {
    out.innerHTML = `<div class="small bad-text">${esc(t("trade.short"))}</div>`;
    return;
  }
  out.innerHTML = `<div class="small">${esc(t("trade.wait"))}</div>`;
  try {
    const opts = {
      maxPrice: Number($("#trade-max")?.value || 0) || null,
      currency: $("#trade-cur")?.value || "exalted",
      category: $("#trade-cat")?.value || "",
      online: $("#trade-online")?.checked ?? true,
    };
    const res = await invoke("trade_search", { query, opts });
    rememberSearch(query);
    renderRecent();
    if (!res.listings.length) {
      out.innerHTML = `<div class="small">${esc(t("trade.empty"))}</div>`;
      return;
    }
    out.innerHTML = `<table class="table">
      <thead><tr><th>${esc(t("trade.price"))}</th><th>${esc(t("trade.item"))}</th><th>${esc(t("trade.account"))}</th><th></th></tr></thead>
      <tbody>${res.listings
        .map(
          (l) => `<tr>
            <td><b>${esc(String(l.price))} ${esc(l.currency)}</b></td>
            <td>${esc(l.name || l.base)}<div class="small">${esc(l.base)}${l.rarity ? " · " + esc(l.rarity) : ""}</div></td>
            <td class="small"><span class="dot-online ${l.online ? "on" : "off"}"></span> ${esc(l.account)}</td>
            <td><div class="row">
              ${l.whisper ? `<button class="btn sm ghost" data-act="trade-copy" data-text="${esc(l.whisper)}">${esc(t("trade.copy"))}</button>` : ""}
              <button class="btn sm ghost" data-act="trade-open" data-url="${esc(res.url)}">${esc(t("trade.open"))}</button>
            </div></td>
          </tr>`
        )
        .join("")}</tbody></table>`;
  } catch (e) {
    out.innerHTML = `<div class="small bad-text">${esc(String(e))}</div>`;
  }
}

function renderHistory() {
  const kinds = ["zone", "level", "reward", "task", "manual", "shot", "ai", "item", "death", "map"];
  const p = profile();
  $("#content").innerHTML = `
    <div class="card tight">
      <div class="row">
        <select id="hist-kind" style="max-width:170px">
          <option value="">${esc(t("hist.all"))}</option>
          ${kinds.map((k) => `<option value="${k}" ${HFILTER.kind === k ? "selected" : ""}>${esc(kindLabel(k))}</option>`).join("")}
        </select>
        <select id="hist-profile" style="max-width:220px">
          <option value="">${esc(t("common.all_profiles"))}</option>
          ${S.saved.profiles.map((pr) => `<option value="${esc(pr.id)}" ${HFILTER.profile === pr.id ? "selected" : ""}>${esc(pr.name || t("common.unknown"))}</option>`).join("")}
        </select>
        <input type="text" id="hist-query" placeholder="${esc(t("hist.search"))}" value="${esc(HFILTER.query)}" style="max-width:260px">
        <button class="btn ghost" data-act="history-export">${esc(t("hist.export"))}</button>
      </div>
      <div class="small" style="margin-top:6px">${esc(t("hist.note"))}</div>
    </div>
    <div class="card" style="padding:0;overflow:hidden">
      <div id="hist-table"></div>
    </div>`;
  const kind = $("#hist-kind");
  const prof = $("#hist-profile");
  const query = $("#hist-query");
  kind?.addEventListener("change", () => {
    HFILTER.kind = kind.value;
    renderHistoryTable();
  });
  prof?.addEventListener("change", () => {
    HFILTER.profile = prof.value;
    renderHistoryTable();
  });
  query?.addEventListener("input", () => {
    HFILTER.query = query.value;
    renderHistoryTable();
  });
  loadHistory(2000).then(renderHistoryTable);
}

function renderHistoryTable() {
  const box = $("#hist-table");
  if (!box) return;
  const rows = HISTORY.filter(
    (e) =>
      (!HFILTER.kind || e.kind === HFILTER.kind) &&
      (!HFILTER.profile || e.profile === HFILTER.profile) &&
      (!HFILTER.query || e.title.toLowerCase().includes(HFILTER.query.toLowerCase()) || e.zone.toLowerCase().includes(HFILTER.query.toLowerCase()))
  )
    .slice()
    .reverse();
  if (!rows.length) {
    box.innerHTML = `<div class="small" style="padding:14px">${esc(t("hist.empty"))}</div>`;
    return;
  }
  box.innerHTML = `<div class="row" style="padding:8px 12px"><span class="small">${esc(t("hist.count"))}: ${rows.length}</span></div>
    <table class="table">
      <thead><tr><th>${esc(t("hist.when"))}</th><th>${esc(t("hist.kind"))}</th><th>${esc(t("hist.lvl"))}</th><th>${esc(t("hist.zone"))}</th><th>${esc(t("hist.title_col"))}</th><th>${esc(t("hist.detail"))}</th></tr></thead>
      <tbody>${rows
        .map(
          (e) => `<tr>
            <td class="mono nowrap">${fmtTime(e.ts)}</td>
            <td class="nowrap">${esc(kindLabel(e.kind))}</td>
            <td class="mono">${e.level || ""}</td>
            <td>${esc(e.zone)}</td>
            <td>${esc(e.title)}</td>
            <td class="small">${esc(e.detail)}</td>
          </tr>`
        )
        .join("")}</tbody>
    </table>`;
}

async function loadHistory(limit = 500) {
  try {
    HISTORY = await invoke("history_list", { filter: { limit } });
  } catch (err) {
    HISTORY = [];
  }
  historyLoaded = true;
}

async function ensureHistory() {
  if (!historyLoaded) await loadHistory(200);
}

// ---------------------------------------------------------------- screenshots
function renderShots() {
  const kindOptions = SHOT_KINDS.map((k) => `<option value="${k}">${esc(t("shot.kind." + k))}</option>`).join("");
  $("#content").innerHTML = `
    <div class="card tight">
      <div class="row between">
        <div class="row">
          <select id="shot-kind" style="max-width:200px" title="${esc(t("shot.kind"))}">${kindOptions}</select>
          <button class="btn primary" data-act="shot-take">${esc(t("shot.take"))}</button>
          <button class="btn ghost" data-act="shots-reveal">${esc(t("shot.open_dir"))}</button>
        </div>
        <span class="small">${esc(t("shot.note"))}</span>
      </div>
    </div>
    <div id="shot-grid" class="shots"></div>`;
  loadShots();
}

async function loadShots() {
  try {
    SHOTS = await invoke("shots_list");
  } catch {
    SHOTS = [];
  }
  const grid = $("#shot-grid");
  if (!grid) return;
  if (!SHOTS.length) {
    grid.innerHTML = `<div class="card"><div class="small">${esc(t("shot.empty"))}</div></div>`;
    return;
  }
  grid.innerHTML = SHOTS.map(
    (s) => `<div class="shot" data-act="shot-open" data-id="${esc(s.id)}">
      <img data-thumb="${esc(s.id)}" alt="">
      <div class="meta">
        <b>${esc(t("shot.kind." + s.kind))}</b>
        <span>${fmtTime(s.ts)} · ${esc(s.zone || "")} ${s.level ? "· " + s.level : ""}</span>
        ${s.note ? `<div class="note">${esc(s.note)}</div>` : ""}
      </div>
    </div>`
  ).join("");
  for (const s of SHOTS) {
    const img = grid.querySelector(`img[data-thumb="${CSS.escape(s.id)}"]`);
    if (!img) continue;
    if (THUMBS.has(s.id)) img.src = THUMBS.get(s.id);
    else
      invoke("shot_thumb", { id: s.id })
        .then((url) => {
          THUMBS.set(s.id, url);
          img.src = url;
        })
        .catch(() => {});
  }
}

async function openShot(id) {
  const s = SHOTS.find((x) => x.id === id);
  if (!s) return;
  activeShot = s;
  const url = await invoke("shot_full", { id }).catch(() => "");
  $("#modal-title").textContent = t("shot.preview");
  $("#modal-body").innerHTML = `
    <img src="${url}" alt="">
    <div class="grid c2">
      <div class="field"><label>${esc(t("shot.kind"))}</label>
        <select id="ms-kind">${SHOT_KINDS.map((k) => `<option value="${k}" ${k === s.kind ? "selected" : ""}>${esc(t("shot.kind." + k))}</option>`).join("")}</select></div>
      <div class="field"><label>${esc(t("shot.note_label"))}</label>
        <input type="text" id="ms-note" maxlength="500" value="${esc(s.note)}"></div>
    </div>
    <dl class="kv" style="margin-top:10px">
      <dt>${esc(t("shot.size"))}</dt><dd class="mono">${s.w}×${s.h}</dd>
      <dt>${esc(t("shot.hash"))}</dt><dd class="mono" style="word-break:break-all;font-size:11px">${esc(s.sha256)}</dd>
      <dt>${esc(t("hist.zone"))}</dt><dd>${esc(s.zone || "—")} ${s.level ? "· " + s.level : ""}</dd>
    </dl>
    <div class="field" style="margin-top:10px"><label>${esc(t("ai.question"))}</label>
      <input type="text" id="ms-question" maxlength="1000" placeholder="${esc(t("ai.question"))}"></div>
    <div class="row" style="margin-top:10px">
      <button class="btn" data-act="ai-run" data-task="analyse_item">${esc(t("ai.analyze_item"))}</button>
      <button class="btn" data-act="ai-run" data-task="next_upgrades">${esc(t("ai.next_upgrades"))}</button>
      <button class="btn" data-act="ai-run" data-task="compare_to_build">${esc(t("ai.compare_build"))}</button>
      <button class="btn ghost" data-act="shot-ai">${esc(t("shot.ai_prepare"))}</button>
    </div>
    <div id="ms-ai-answer" class="ai-answer" hidden></div>
    <div class="row" style="margin-top:12px">
      <button class="btn primary" data-act="shot-save">${esc(t("shot.save"))}</button>
      <button class="btn danger" data-act="shot-delete">${esc(t("shot.delete"))}</button>
      <span class="spacer"></span>
      <button class="btn ghost" data-act="modal-close">${esc(t("common.close"))}</button>
    </div>`;
  $("#modal").hidden = false;
}

async function runAi(task) {
  if (!activeShot) return;
  const box = $("#ms-ai-answer");
  if (!box) return;
  box.hidden = false;
  box.innerHTML = `<div class="small">${esc(t("ai.waiting"))}</div>`;
  try {
    const res = await invoke("ai_analyze", {
      ids: [activeShot.id],
      task,
      question: ($("#ms-question")?.value || "").trim(),
    });
    const meta = [
      `${esc(t("ai.usage"))}: ${res.prompt_tokens} → ${res.completion_tokens}`,
      res.image_sent ? t("ai.image_sent") : t("ai.image_not_sent"),
      `${res.provider} · ${esc(res.model)}`,
    ].join(" · ");
    box.innerHTML = `<h3>${esc(t("ai.answer"))}</h3>
      <div class="ai-text">${esc(res.text).replaceAll("\n", "<br>")}</div>
      ${res.reasoning ? `<details style="margin-top:8px"><summary class="small">${esc(t("ai.reasoning"))}</summary><div class="small prewrap" style="margin-top:6px">${esc(res.reasoning)}</div></details>` : ""}
      <div class="small" style="margin-top:8px">${meta}</div>
      <div class="small">${esc(t("ai.manifest", { path: res.manifest }))}</div>`;
  } catch (err) {
    box.innerHTML = `<div class="small bad-text">${esc(t("ai.failed"))}: ${esc(String(err))}</div>`;
  }
}

const SLOT_RU = {
  amulet: "Амулет",
  belt: "Пояс",
  body: "Нагрудник",
  boots: "Ботинки",
  gloves: "Перчатки",
  helmet: "Шлем",
  leftRing: "Кольцо",
  rightRing: "Кольцо",
  extraRing: "Кольцо",
  mainHand: "Оружие",
  offHand: "Оффхенд",
  flask1: "Флакон",
  flask2: "Флакон",
  charm1: "Талисман",
  charm2: "Талисман",
  charm3: "Талисман",
};

const SLOT_EN = {
  amulet: "Amulet",
  belt: "Belt",
  body: "Body armour",
  boots: "Boots",
  gloves: "Gloves",
  helmet: "Helmet",
  leftRing: "Ring",
  rightRing: "Ring",
  extraRing: "Ring",
  mainHand: "Weapon",
  offHand: "Off-hand",
  flask1: "Flask",
  flask2: "Flask",
  charm1: "Charm",
  charm2: "Charm",
  charm3: "Charm",
};

function slotLabel(slot) {
  const key = String(slot).replace(/-\d+$/, "");
  if (S.saved.lang === "ru") return SLOT_RU[key] || key;
  return SLOT_EN[key] || key;
}

function openGuideImport() {
  $("#modal-title").textContent = t("build.import_url_title");
  $("#modal-body").innerHTML = `
    <div class="small" style="margin-bottom:8px">${esc(t("build.import_url_hint"))}</div>
    <input type="text" id="guide-url" style="width:100%" placeholder="https://mobalytics.gg/poe-2/builds/...">
    <div class="row" style="margin-top:10px">
      <button class="btn ghost" data-act="guide-variants">${esc(t("build.import_check"))}</button>
      <button class="btn primary" data-act="guide-run">${esc(t("build.import_url_run"))}</button>
      <button class="btn ghost" data-act="modal-close">${esc(t("common.close"))}</button>
      <span class="small bad-text" id="guide-error"></span>
    </div>
    <div class="field" id="guide-variant-wrap" hidden style="margin-top:8px">
      <label>${esc(t("build.import_variant"))}</label>
      <select id="guide-variant"></select>
    </div>
    <div class="small" id="guide-wait" hidden style="margin-top:8px">${esc(t("build.import_url_wait"))}</div>
    <div class="small good-text" id="guide-stage" hidden style="margin-top:4px"></div>`;
  $("#modal").hidden = false;
}

async function loadGuideVariants() {
  const url = $("#guide-url")?.value.trim() || "";
  const err = $("#guide-error");
  if (err) err.textContent = "";
  const wait = $("#guide-wait");
  if (wait) wait.hidden = false;
  const stage = $("#guide-stage");
  if (stage) {
    stage.hidden = true;
    stage.textContent = "";
  }
  try {
    const variants = await invoke("guide_variants", { url });
    const select = $("#guide-variant");
    const wrap = $("#guide-variant-wrap");
    if (select && wrap) {
      select.innerHTML = variants
        .map((v) => `<option value="${v.index}">${esc(v.label)} — ${v.skills} ${esc(t("guide.skills"))}, ${v.gear} ${esc(t("guide.gear"))}</option>`)
        .join("");
      wrap.hidden = false;
    }
  } catch (e) {
    if (err) err.textContent = String(e);
  } finally {
    if (wait) wait.hidden = true;
    if (stage) stage.hidden = true;
  }
}

async function runGuideImport() {
  const url = $("#guide-url")?.value.trim() || "";
  const err = $("#guide-error");
  const wait = $("#guide-wait");
  if (err) err.textContent = "";
  if (wait) wait.hidden = false;
  const stage = $("#guide-stage");
  if (stage) {
    stage.hidden = true;
    stage.textContent = "";
  }
  try {
    const variantSelect = $("#guide-variant");
    const variant = variantSelect && !$("#guide-variant-wrap")?.hidden ? Number(variantSelect.value) : null;
    const doc = await invoke("build_import_url", { url, variant });
    BUILDS = BUILDS.filter((b) => b.id !== doc.id);
    BUILDS.push(doc);
    await invoke("build_select", { id: doc.id });
    $("#modal").hidden = true;
    toast(`${t("build.import_url_done")}: ${l10n(doc.name)}`);
    renderSection();
  } catch (e) {
    if (err) err.textContent = String(e);
  } finally {
    if (wait) wait.hidden = true;
    if (stage) stage.hidden = true;
  }
}

let UPGRADE_UNLISTEN = null;

async function runUpgrades(gid) {
  const box = $("#guide-upgrades");
  if (!box) return;
  const budget = Number($("#upg-budget")?.value || 0) || null;
  box.innerHTML = `<div class="small" id="upg-progress">${esc(t("guide.upgrade_hint"))}</div>`;
  if (!UPGRADE_UNLISTEN) {
    UPGRADE_UNLISTEN = await listen("upgrade-progress", (e) => {
      const p = e.payload || {};
      const line = $("#upg-progress");
      if (line) line.textContent = `${t("guide.upgrade_progress")} ${p.done}/${p.total}${p.slot ? " · " + slotLabel(p.slot) : ""}`;
    });
  }
  try {
    const rows = await invoke("guide_upgrade_list", { guideId: gid, maxDivine: budget });
    rows.sort((a, b) => (a.divine ?? 1e9) - (b.divine ?? 1e9) || a.min - b.min);
    const money = (n) => (Number.isInteger(n) ? String(n) : n.toFixed(1));
    const total = rows.reduce((sum, r) => sum + (r.divine ?? 0), 0);
    const withDivine = rows.filter((r) => r.divine != null).length;
    box.innerHTML = `<h3 style="margin-top:10px">${esc(t("guide.upgrades"))}</h3>
      <table class="table"><thead><tr><th>${esc(t("guide.slot"))}</th><th>${esc(t("trade.item"))}</th><th>${esc(t("guide.price_range"))}</th><th>divine</th><th></th></tr></thead>
      <tbody>${rows
        .map(
          (r) => `<tr>
            <td class="small">${esc(slotLabel(r.slot))}</td>
            <td><b>${esc(r.name)}</b>${r.over_budget ? ` <span class="tag boss">${esc(t("guide.over"))}</span>` : ""}</td>
            <td class="small">${r.error ? `<span class="bad-text">${esc(r.error)}</span>` : `${money(r.min)} / ${money(r.median)} ${esc(r.currency)} <span class="small">(${r.total})</span>`}</td>
            <td class="small">${r.divine != null ? "≈ " + money(r.divine) : "—"}</td>
            <td>${r.url ? `<button class="link" data-act="link" data-url="${esc(r.url)}">${esc(t("items.price_open"))}</button>` : ""}</td>
          </tr>`
        )
        .join("")}</tbody></table>
      <div class="small" style="margin-top:6px">${esc(t("guide.total"))}: <b>≈ ${money(total)} divine</b>${withDivine < rows.length ? ` (${withDivine}/${rows.length})` : ""}${budget ? ` · ${esc(t("guide.budget"))}: ${money(budget)}` : ""}</div>`;
  } catch (e) {
    if (box) box.innerHTML = `<div class="small bad-text">${esc(String(e))}</div>`;
  }
}

function renderGuide(g) {
  let card = $("#guide-card");
  if (!card) {
    card = document.createElement("div");
    card.id = "guide-card";
    card.className = "card";
    $("#content").appendChild(card);
  }
  const owned = new Set(g.owned || []);
  const missingGems = (g.skills || []).filter((s) => !owned.has("gem:" + s.main)).length;
  const missingGear = (g.gear || []).filter((it) => !owned.has("gear:" + it.slot)).length;
  const summary =
    missingGems + missingGear === 0
      ? `<span class="good-text">${esc(t("guide.all_have"))}</span>`
      : `${esc(t("guide.missing"))}: <b>${missingGems}</b> ${esc(t("guide.missing_gems"))} · <b>${missingGear}</b> ${esc(t("guide.missing_items"))}`;

  card.hidden = false;
  card.innerHTML = `
    <div class="row between">
      <h2>${esc(t("guide.title"))}</h2>
      <div class="row">
        ${g.url ? `<button class="btn sm ghost" data-act="guide-refresh" data-guide="${esc(g.id)}" title="${esc(t("guide.refresh_hint"))}">${esc(t("guide.refresh"))}</button>
        <button class="link" data-act="link" data-url="${esc(g.url)}">${esc(g.url)}</button>` : ""}
      </div>
    </div>
    <div class="small">${esc(g.class || "")}${g.ascendancy ? " · " + esc(g.ascendancy) : ""}${g.variant ? " · " + esc(g.variant) : ""}</div>
    <div class="row between" style="margin-top:6px">
      <span class="small">${summary}</span>
      <span class="small">${esc(t("guide.owned_hint"))}</span>
    </div>
    <div class="row" style="margin-top:8px">
      <button class="btn sm ghost" data-act="guide-upgrades" data-guide="${esc(g.id)}">${esc(t("guide.upgrades"))}</button>
      <input id="upg-budget" type="number" min="0" step="1" placeholder="${esc(t("guide.budget"))}" style="width:150px">
    </div>
    <div id="guide-upgrades"></div>
    <h3 style="margin-top:10px">${esc(t("guide.skills"))}</h3>
    <div>${(g.skills || [])
      .map((s) => {
        const key = "gem:" + s.main;
        const has = owned.has(key);
        return `<label class="check-item ${has ? "done" : ""}">
          <input type="checkbox" data-act="guide-owned" data-guide="${esc(g.id)}" data-key="${esc(key)}" ${has ? "checked" : ""}>
          <span><b>${esc(s.main)}</b>${s.supports.length ? " + " + esc(s.supports.join(", ")) : ""}</span>
        </label>`;
      })
      .join("")}</div>
    <h3 style="margin-top:12px">${esc(t("guide.gear"))}</h3>
    <table class="table">
      <thead><tr><th>${esc(t("guide.have"))}</th><th>${esc(t("guide.slot"))}</th><th>${esc(t("trade.item"))}</th><th></th></tr></thead>
      <tbody>${(g.gear || [])
        .map((it) => {
          const key = "gear:" + it.slot;
          const has = owned.has(key);
          return `<tr class="${has ? "done" : ""}">
            <td><input type="checkbox" data-act="guide-owned" data-guide="${esc(g.id)}" data-key="${esc(key)}" ${has ? "checked" : ""}></td>
            <td class="small">${esc(slotLabel(it.slot))}</td>
            <td><b>${esc(it.name)}</b><div class="small">${esc((it.mods || []).slice(0, 3).join(" · "))}</div></td>
            <td><button class="btn sm ghost" data-act="guide-find" data-guide="${esc(g.id)}" data-slot="${esc(it.slot)}">${esc(t("guide.buy"))}</button>
              <div class="guide-out small" style="margin-top:4px"></div></td>
          </tr>`;
        })
        .join("")}</tbody>
    </table>`;
}

async function loadGuide(buildId) {
  try {
    const g = await invoke("guide_for_build", { buildId });
    if (g) renderGuide(g);
  } catch {}
}

function openPobImport() {
  $("#modal-title").textContent = t("build.import_pob_title");
  $("#modal-body").innerHTML = `
    <div class="small" style="margin-bottom:8px">${esc(t("build.import_pob_hint"))}</div>
    <textarea id="pob-code" rows="7" placeholder="${esc(t("build.import_pob_placeholder"))}"></textarea>
    <div class="row" style="margin-top:10px">
      <button class="btn primary" data-act="pob-run">${esc(t("build.import_pob_run"))}</button>
      <button class="btn ghost" data-act="modal-close">${esc(t("common.close"))}</button>
      <span class="small bad-text" id="pob-error"></span>
    </div>`;
  $("#modal").hidden = false;
}

async function runPobImport() {
  const code = $("#pob-code")?.value || "";
  const err = $("#pob-error");
  if (err) err.textContent = "";
  try {
    const doc = await invoke("build_import_pob", { code });
    BUILDS = BUILDS.filter((b) => b.id !== doc.id);
    BUILDS.push(doc);
    await invoke("build_select", { id: doc.id });
    $("#modal").hidden = true;
    toast(`${t("build.import_pob_done")}: ${l10n(doc.name)}`);
    renderSection();
  } catch (e) {
    if (err) err.textContent = `${t("build.import_pob_failed")}: ${String(e)}`;
  }
}

async function prepareAi() {
  if (!activeShot) return;
  try {
    const res = await invoke("ai_prepare", { ids: [activeShot.id], task: "analyse_item" });
    toast(`${t("shot.ai_done")}: ${res.path}`);
    $("#modal-body").insertAdjacentHTML("beforeend", `<pre class="diag" style="margin-top:12px">${esc(res.json)}</pre>`);
  } catch (err) {
    toast(String(err), true);
  }
}

// ---------------------------------------------------------------- settings
function renderSettings() {
  const o = S.saved.overlay;
  const hk = S.saved.hotkeys;
  const log = S.saved.log;
  const hkRows = [
    ["next", "set.hk_next"],
    ["prev", "set.hk_prev"],
    ["mode", "set.hk_mode"],
    ["hide", "set.hk_hide"],
    ["screenshot", "set.hk_screenshot"],
    ["main", "set.hk_main"],
    ["item", "set.hk_item"],
    ["regex", "set.hk_regex"],
    ["map", "set.hk_map"],
    ["price", "set.hk_price"],
  ];
  const providerOptions = [{ id: "disabled", label: t("common.none") }, ...(S.ai_providers || [])]
    .map((p) => `<option value="${esc(p.id)}" ${p.id === (S.saved.ai.provider || "disabled") ? "selected" : ""}>${esc(p.label)}</option>`)
    .join("");

  $("#content").innerHTML = `
  <div class="settings">
    <div class="card">
      <h2>${esc(t("set.interface"))}</h2>
      <div class="row">
        <label class="check"><input type="radio" name="lang" value="ru" ${S.saved.lang !== "en" ? "checked" : ""}> ${esc(t("set.lang_ru"))}</label>
        <label class="check"><input type="radio" name="lang" value="en" ${S.saved.lang === "en" ? "checked" : ""}> ${esc(t("set.lang_en"))}</label>
        <span class="spacer"></span>
        <span class="small">${esc(t("set.trust"))}</span>
      </div>
    </div>

    <div class="card">
      <h2>${esc(t("set.log"))}</h2>
      <div class="grid c2">
        <div class="field"><label>${esc(t("set.log_mode"))}</label>
          <select id="st-log-mode">
            <option value="auto" ${log.mode !== "manual" ? "selected" : ""}>${esc(t("set.log_auto"))}</option>
            <option value="manual" ${log.mode === "manual" ? "selected" : ""}>${esc(t("set.log_manual"))}</option>
          </select></div>
        <div class="field"><label>${esc(t("set.log_current"))}</label>
          <input type="text" value="${esc(S.log_path || "—")}" readonly>
          <span class="small">${esc(t("set.log_enc"))}: ${esc(S.log_encoding || "—")}</span></div>
      </div>
      <div class="field" style="margin-top:10px"><label>${esc(t("set.log_path"))}</label>
        <div class="row"><input type="text" id="st-log-path" style="flex:1;min-width:180px" value="${esc(log.path || "")}" placeholder="C:\\Program Files (x86)\\Grinding Gear Games\\Path of Exile 2\\logs\\Client.txt">
          <button class="btn sm" data-act="pick-log">${esc(t("set.browse"))}</button></div></div>
      <div class="row" style="margin-top:8px">
        <button class="btn" data-act="log-check">${esc(t("set.log_check"))}</button>
        <button class="btn ghost" data-act="log-auto">${esc(t("set.log_auto_btn"))}</button>
        <span id="log-check-result" class="small"></span>
      </div>
    </div>

    <div class="card">
      <h2>${esc(t("set.overlay"))}</h2>
      <div class="grid c3">
        <div class="field"><label>${esc(t("set.overlay_mode"))}</label>
          <select id="st-mode">
            ${["compact", "standard", "extended"].map((m) => `<option value="${m}" ${o.mode === m ? "selected" : ""}>${esc(t("set.overlay_" + m))}</option>`).join("")}
          </select></div>
        <label class="check"><input type="checkbox" id="st-visible" ${o.visible ? "checked" : ""}> ${esc(t("set.visible"))}</label>
        <label class="check"><input type="checkbox" id="st-only-game" ${o.only_in_game ? "checked" : ""}> ${esc(t("set.only_in_game"))}</label>
      </div>
      <div class="slider" style="margin-top:12px"><span>${esc(t("set.opacity"))}</span>
        <input type="range" id="st-opacity" min="0" max="100" step="2" value="${o.opacity}"><output id="st-opacity-v">${o.opacity}%</output></div>
      <div class="slider"><span>${esc(t("set.scale"))}</span>
        <input type="range" id="st-scale" min="60" max="200" step="5" value="${o.scale}"><output id="st-scale-v">${o.scale}%</output></div>
      <div class="row" style="margin-top:10px">
        <button class="btn sm ghost" data-act="reset-pos">${esc(t("set.reset_pos"))}</button>
        <span class="small">x=${o.x}, y=${o.y}</span>
      </div>
    </div>

    <div class="card">
      <h2>${esc(t("set.hotkeys"))}</h2>
      <div class="small" style="margin-bottom:8px">${esc(t("set.hotkeys_hint"))}</div>
      <div class="hk-grid">
        ${hkRows.map(([key, label]) => `<span>${esc(t(label))}</span><button type="button" class="btn hk" data-hk="${key}" data-value="${esc(hk[key] || "")}">${esc(prettyHotkey(hk[key]))}</button>`).join("")}
      </div>
      ${S.hotkey_error ? `<div class="small bad-text" style="margin-top:8px">${esc(S.hotkey_error)}</div>` : ""}
    </div>

    <div class="card">
      <h2>${esc(t("set.route"))}</h2>
      <div class="row">
        <label class="check"><input type="radio" name="route" value="all" ${S.saved.route !== "must" ? "checked" : ""}> ${esc(t("camp.route_all"))}</label>
        <label class="check"><input type="radio" name="route" value="must" ${S.saved.route === "must" ? "checked" : ""}> ${esc(t("camp.route_must"))}</label>
      </div>
    </div>

    <div class="card">
      <h2>${esc(t("set.shots"))}</h2>
      <div class="field"><label>${esc(t("set.shots_dir"))}</label>
        <div class="row"><input type="text" id="st-shots-dir" style="flex:1;min-width:180px" value="${esc(S.saved.shots_dir || "")}" placeholder="${esc(S.shots_dir)}">
          <button class="btn sm" data-act="pick-shots-dir">${esc(t("set.browse"))}</button></div>
        <span class="small">${esc(t("set.shots_dir_hint"))} ${esc(t("set.log_current"))}: ${esc(S.shots_dir)}</span></div>
      <div class="row" style="margin-top:8px"><button class="btn ghost" data-act="shots-reveal">${esc(t("shot.open_dir"))}</button></div>
    </div>

    <div class="card aliases">
      <h2>${esc(t("set.aliases"))}</h2>
      <div class="small" style="margin-bottom:8px">${esc(t("set.aliases_hint"))}</div>
      <div id="alias-rows">
        ${(S.saved.zone_aliases || []).map((a, i) => aliasRow(a, i)).join("")}
      </div>
      <button class="btn sm ghost" data-act="alias-add" style="margin-top:6px">+ ${esc(t("set.add_alias"))}</button>
    </div>

    <div class="card aliases">
      <h2>${esc(t("set.regexes"))}</h2>
      <div class="small" style="margin-bottom:8px">${esc(t("set.regexes_hint"))}</div>
      <div id="regex-rows">
        ${(S.saved.regexes || []).map((r) => regexRow(r)).join("")}
      </div>
      <button class="btn sm ghost" data-act="regex-add" style="margin-top:6px">+ ${esc(t("set.regex_add"))}</button>
    </div>

    <div class="card">
      <h2>${esc(t("set.map"))}</h2>
      <div class="small" style="margin-bottom:8px">${esc(t("set.map_hint"))}</div>
      <label class="check"><input type="checkbox" id="st-map-enabled" ${S.saved.map?.enabled ? "checked" : ""}> ${esc(t("set.map_enable"))}</label>
      <div class="field" style="margin-top:8px"><label>${esc(t("set.map_mechanics"))}</label>
        <input type="text" id="st-map-mechanics" value="${esc((S.saved.map?.mechanics || []).join(", "))}" placeholder="Breach, Ritual, Expedition, Delirium, Tower"></div>
    </div>

    <div class="card">
      <h2>${esc(t("set.trade"))}</h2>
      <div class="small" style="margin-bottom:8px">${esc(t("set.trade_hint"))}</div>
      <div class="grid c2">
        <label class="check"><input type="checkbox" id="st-trade-enabled" ${S.saved.trade?.enabled ? "checked" : ""}> ${esc(t("set.trade_enable"))}</label>
        <div class="field"><label>${esc(t("set.trade_league"))}</label>
          <input type="text" id="st-trade-league" value="${esc(S.saved.trade?.league || "")}" placeholder="Standard"></div>
        <div class="field"><label>${esc(t("set.trade_filters"))}</label>
          <select id="st-trade-filters">${[0, 1, 2, 3].map((n) => `<option value="${n}" ${n === (S.saved.trade?.filters ?? 2) ? "selected" : ""}>${n}</option>`).join("")}</select>
          <div class="small">${esc(t("set.trade_filters_hint"))}</div>
        </div>
        <div class="field"><label>${esc(t("set.trade_headroom"))}</label>
          <select id="st-trade-headroom">${[0, 5, 10, 15, 20].map((n) => `<option value="${n}" ${n === (S.saved.trade?.headroom ?? 10) ? "selected" : ""}>${n}%</option>`).join("")}</select>
        </div>
      </div>
    </div>

    <div class="card">
      <h2>${esc(t("set.ai"))}</h2>
      <div class="small" style="margin-bottom:8px">${esc(t("set.ai_hint"))}</div>
      <div class="grid c3">
        <div class="field"><label>${esc(t("set.ai_provider"))}</label>
          <select id="st-ai-provider">${providerOptions}</select></div>
        <div class="field"><label>${esc(t("set.ai_model"))}</label>
          <input type="text" id="st-ai-model" list="ai-models" value="${esc(S.saved.ai.model || "")}" placeholder="${esc(providerInfo(S.saved.ai.provider)?.default_model || "deepseek-chat")}">
          <datalist id="ai-models"></datalist>
          <span class="small">${esc(t("set.ai_model_hint"))}</span></div>
        <div class="field"><label>${esc(t("set.ai_reasoning"))}</label>
          <select id="st-ai-reasoning">${REASONING.map((r) => `<option value="${r}" ${(S.saved.ai.reasoning || "auto") === r ? "selected" : ""}>${esc(t("set.ai_reasoning_" + r))}</option>`).join("")}</select></div>
        <div class="field"><label>${esc(t("set.ai_key"))}</label>
          <input type="password" id="st-ai-key" placeholder="${esc(S.ai_has_key ? t("set.ai_key_saved") : t("set.ai_key_none"))}"></div>
      </div>
      <div class="row" style="margin-top:8px">
        <label class="check"><input type="checkbox" id="st-ai-enabled" ${S.saved.ai.enabled ? "checked" : ""}> ${esc(t("set.ai_enable"))}</label>
        <button class="btn sm" data-act="ai-test">${esc(t("set.ai_test"))}</button>
        <button class="btn sm danger" data-act="ai-clear-key">${esc(t("set.ai_key_clear"))}</button>
        <span class="small ${S.ai_has_key ? "good-text" : ""}">${esc(S.ai_has_key ? t("set.ai_key_saved") : t("set.ai_key_none"))}</span>
        <span class="small" id="ai-test-result"></span>
      </div>
      <div class="small" style="margin-top:6px" id="ai-vision-hint"></div>
      <div class="row" style="margin-top:6px">
        ${(S.ai_providers || []).map((p) => `<button class="link" data-act="link" data-url="${esc(p.key_url)}">${esc(p.label)} API</button>`).join("")}
      </div>
    </div>

    <div class="card">
      <h2>${esc(t("set.diagnostics"))}</h2>
      <pre class="diag" id="diag-box">…</pre>
      <div class="row" style="margin-top:8px">
        <button class="btn sm" data-act="diag-refresh">${esc(t("set.diag_refresh"))}</button>
        <button class="btn sm ghost" data-act="diag-copy">${esc(t("set.diag_copy"))}</button>
        <button class="btn sm ghost" data-act="data-dir">${esc(t("set.data_dir"))}</button>
      </div>
    </div>

    <div class="card">
      <h2>${esc(t("set.about"))}</h2>
      <dl class="kv">
        <dt>${esc(t("set.version"))}</dt><dd>Eni-PoE2-Companion ${esc(VERSION)} · ${esc(CAMP?.version || "")} · ${esc(CAMP?.patch || "")}</dd>
        <dt>${esc(t("set.license"))}</dt><dd></dd>
      </dl>
      <div class="row" style="margin-top:8px">
        <button class="link" data-act="link" data-url="https://github.com/BigbyOn/Eni-PoE2-Companion">${esc(t("set.repo"))}</button>
        <button class="link" data-act="link" data-url="https://github.com/maluramichael/poe2overlay">${esc(t("set.upstream"))}</button>
      </div>
      <div class="hr"></div>
      <h3>${esc(t("set.limits"))}</h3>
      <ul class="list limits">${[1, 2, 3, 4, 5, 6].map((i) => `<li>${esc(t("limits." + i))}</li>`).join("")}</ul>
    </div>

    <div class="row" style="margin:4px 0 24px">
      <button class="btn primary" data-act="save-settings">${esc(t("set.save"))}</button>
      <button class="btn danger ghost" data-act="quit">${esc(t("set.quit"))}</button>
    </div>
  </div>`;

  const bind = (id, out, suffix = "%") => {
    const el = $(id);
    if (!el) return;
    el.addEventListener("input", () => {
      $(out).textContent = el.value + suffix;
      previewLook();
    });
  };
  bind("#st-opacity", "#st-opacity-v");
  bind("#st-scale", "#st-scale-v");
  $("#st-mode")?.addEventListener("change", previewLook);
  $$(".hk").forEach((btn) =>
    btn.addEventListener("click", () => {
      capturing?.classList.remove("capturing");
      capturing = btn;
      btn.classList.add("capturing");
      btn.textContent = t("set.hotkey_placeholder");
    })
  );
  const syncAi = () => {
    const provider = $("#st-ai-provider")?.value || "disabled";
    const info = providerInfo(provider);
    const list = $("#ai-models");
    if (list) list.innerHTML = (info?.models || []).map((m) => `<option value="${esc(m)}">`).join("");
    const hint = $("#ai-vision-hint");
    if (hint) {
      if (!info) hint.textContent = t("set.ai_off");
      else hint.textContent = aiVision(provider, $("#st-ai-model")?.value || info.default_model) ? t("set.ai_vision") : t("set.ai_no_vision");
    }
  };
  $("#st-ai-provider")?.addEventListener("change", () => {
    const info = providerInfo($("#st-ai-provider").value);
    const model = $("#st-ai-model");
    if (model) model.placeholder = info?.default_model || "";
    syncAi();
  });
  $("#st-ai-model")?.addEventListener("input", syncAi);
  $("#st-ai-enabled")?.addEventListener("change", syncAi);
  syncAi();

  $$("#regex-rows .arow button").forEach((btn) =>
    btn.addEventListener("click", () => {
      btn.closest(".arow").remove();
    })
  );
  $$("#alias-rows .arow button").forEach((btn) =>
    btn.addEventListener("click", () => {
      btn.closest(".arow").remove();
    })
  );
  refreshDiagnostics();
}

function regexRow(r = { name: "", value: "" }) {
  return `<div class="arow">
    <input type="text" class="rname" placeholder="${esc(t("set.regex_name"))}" value="${esc(r.name)}">
    <input type="text" class="rvalue" placeholder="${esc(t("set.regex_value"))}" value="${esc(r.value)}">
    <button class="icon" type="button">✕</button>
  </div>`;
}

function itemCard(rec) {
  const p = rec.parsed || {};
  const a = rec.analysis || {};
  const matches = (a.matches || []).map((m) => `<span class="tag must">${esc(m)}</span>`).join(" ");
  const warnings = (a.warnings || []).map((w) => `<span class="tag boss">${esc(w)}</span>`).join(" ");
  const dangers = (rec.dangers || []).map((d) => `<span class="tag boss">⚠ ${esc(d.label)}</span>`).join(" ");
  const searchName = (p.base || p.name || "").trim();
  const links = searchName
    ? `<div class="row small" style="margin-top:6px;gap:8px">
        <button class="link" data-act="link" data-url="${esc(`https://www.poe2wiki.net/index.php?search=${encodeURIComponent(searchName)}`)}">Wiki</button>
        <button class="link" data-act="link" data-url="${esc(`https://poedb.tw/us/search?q=${encodeURIComponent(searchName)}`)}">PoEDB</button>
        <button class="link" data-act="link" data-url="https://www.craftofexile.com/?game=poe2">Craft of Exile</button>
      </div>`
    : "";
  const history = rec.price_history || [];
  const last = history[history.length - 1];
  const score = a.score || 0;
  const cls = score >= 3 ? "good-text" : score > 0 ? "warn-text" : "bad-text";
  return `<div class="item">
    <div class="row between">
      <div><b>${esc(p.name || p.base)}</b>${p.base && p.base !== p.name ? ` <span class="small">${esc(p.base)}</span>` : ""}</div>
      <div class="row">
        <span class="small ${cls}">${esc(t("items.score"))}: ${score}</span>
        ${S.saved.trade?.enabled ? `<button class="btn sm ghost" data-act="item-price" data-id="${esc(rec.id)}">${esc(t("items.price"))}</button>` : ""}
        <button class="btn sm ghost" data-act="item-ai" data-id="${esc(rec.id)}">${esc(t("items.ai"))}</button>
        <button class="btn sm danger" data-act="item-delete" data-id="${esc(rec.id)}">${esc(t("items.delete"))}</button>
      </div>
    </div>
    <div class="small">${esc(p.rarity || "")}${p.slot ? " · " + esc(p.slot) : ""}${p.item_level ? ` · ${esc(t("items.ilvl"))} ${p.item_level}` : ""} · ${fmtTime(rec.ts)}</div>
    <div class="chips" style="margin-top:5px">${matches}${warnings}</div>
    ${dangers ? `<div class="chips" style="margin-top:4px">${dangers}</div>` : ""}
    ${last ? `<div class="small">${esc(t("items.last_price"))}: <b>${last.min}</b> ${esc(last.currency)} · ${fmtTime(last.ts)}${history.length > 1 ? ` · ${esc(t("items.checks"))}: ${history.length}` : ""}</div>` : ""}
    <div class="price-slot" style="margin-top:6px" hidden></div>
    <div class="ai-slot" style="margin-top:6px" ${rec.ai ? "" : "hidden"}>
      ${rec.ai ? `<div class="ai-answer"><div class="small">${esc(t("ai.answer"))}</div><div class="ai-text">${esc(rec.ai).replaceAll("\n", "<br>")}</div></div>` : ""}
    </div>
    ${links}
    <details style="margin-top:6px"><summary class="small">${esc(t("items.raw"))}</summary><pre class="diag" style="max-height:180px;margin-top:4px">${esc(rec.raw)}</pre></details>
  </div>`;
}

function renderItemList() {
  const box = $("#item-list");
  if (!box) return;
  box.innerHTML = ITEMS.length ? ITEMS.map(itemCard).join("") : `<div class="small">${esc(t("items.empty"))}</div>`;
}

async function loadItems() {
  try {
    ITEMS = await invoke("items_list");
  } catch {
    ITEMS = [];
  }
  renderItemList();
}

function since(iso) {
  if (!iso) return "—";
  const secs = Math.max(0, Math.floor((Date.now() - new Date(iso).getTime()) / 1000));
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  return h ? `${h} ч ${String(m).padStart(2, "0")} мин` : m ? `${m} мин ${String(s).padStart(2, "0")} с` : `${s} с`;
}

function updateSessionClocks() {
  const e = $("#sess-elapsed");
  if (e) e.textContent = since(S.session_started);
  const z = $("#sess-zone");
  if (z) z.textContent = since(S.zone_since);
}

function aliasRow(a = { alias: "", zone: "" }, i = "") {
  return `<div class="arow" data-i="${esc(i)}">
    <input type="text" class="alias" placeholder="${esc(t("set.alias_from"))}" value="${esc(a.alias)}">
    <input type="text" class="zone" placeholder="${esc(t("set.alias_to"))}" value="${esc(a.zone)}">
    <button class="icon" type="button">✕</button>
  </div>`;
}

function prettyHotkey(k) {
  if (!k) return t("common.none");
  return String(k)
    .replace(/Key([A-Z])/g, "$1")
    .replace(/Digit(\d)/g, "$1")
    .replace("ArrowRight", "→")
    .replace("ArrowLeft", "←")
    .replace("ArrowUp", "↑")
    .replace("ArrowDown", "↓")
    .replace(/\+/g, " + ");
}

function previewLook() {
  const mode = $("#st-mode")?.value || S.saved.overlay.mode;
  const scale = Number($("#st-scale")?.value || S.saved.overlay.scale);
  const opacity = Number($("#st-opacity")?.value ?? S.saved.overlay.opacity);
  invoke("preview_look", { look: { scale, opacity, mode } });
}

async function refreshDiagnostics() {
  try {
    const text = await invoke("diagnostics");
    const box = $("#diag-box");
    if (box) box.textContent = text;
  } catch (err) {
    const box = $("#diag-box");
    if (box) box.textContent = String(err);
  }
}

function collectHotkeys() {
  const out = {};
  $$(".hk").forEach((b) => (out[b.dataset.hk] = b.dataset.value || ""));
  return out;
}

function baseSettings() {
  return {
    lang: S.saved.lang,
    route: S.saved.route,
    hotkeys: S.saved.hotkeys,
    log: S.saved.log,
    overlay: S.saved.overlay,
    shots_dir: S.saved.shots_dir,
    zone_aliases: S.saved.zone_aliases,
    regexes: S.saved.regexes,
    ai_provider: S.saved.ai.provider,
    ai_model: S.saved.ai.model,
    ai_reasoning: S.saved.ai.reasoning || "auto",
    ai_enabled: S.saved.ai.enabled,
    ai_key: "",
    ai_clear_key: false,
    trade_enabled: Boolean(S.saved.trade?.enabled),
    trade_league: S.saved.trade?.league || "",
    trade_filters: S.saved.trade?.filters ?? 2,
    trade_headroom: S.saved.trade?.headroom ?? 10,
    map_enabled: Boolean(S.saved.map?.enabled),
    map_mechanics: S.saved.map?.mechanics || [],
  };
}

function collectSettings({ keepKey = false } = {}) {
  const overlay = {
    mode: $("#st-mode")?.value || S.saved.overlay.mode,
    x: S.saved.overlay.x,
    y: S.saved.overlay.y,
    opacity: Number($("#st-opacity")?.value ?? S.saved.overlay.opacity),
    scale: Number($("#st-scale")?.value ?? S.saved.overlay.scale),
    visible: $("#st-visible")?.checked ?? true,
    only_in_game: $("#st-only-game")?.checked ?? true,
  };
  const aliases = $$("#alias-rows .arow")
    .map((row) => ({ alias: row.querySelector(".alias").value.trim(), zone: row.querySelector(".zone").value.trim() }))
    .filter((a) => a.alias && a.zone);
  return {
    lang: $('input[name="lang"]:checked')?.value || "ru",
    route: $('input[name="route"]:checked')?.value || "all",
    hotkeys: collectHotkeys(),
    log: { mode: $("#st-log-mode")?.value || "auto", path: $("#st-log-path")?.value.trim() || null },
    overlay,
    shots_dir: $("#st-shots-dir")?.value.trim() || null,
    zone_aliases: aliases,
    regexes: $$("#regex-rows .arow")
      .map((row) => ({ name: row.querySelector(".rname").value.trim(), value: row.querySelector(".rvalue").value.trim() }))
      .filter((r) => r.value),
    ai_provider: $("#st-ai-provider")?.value || "disabled",
    ai_model: $("#st-ai-model")?.value.trim() || "",
    ai_reasoning: $("#st-ai-reasoning")?.value || "auto",
    ai_enabled: $("#st-ai-enabled")?.checked ?? false,
    ai_key: keepKey ? "" : $("#st-ai-key")?.value || "",
    ai_clear_key: false,
    trade_enabled: $("#st-trade-enabled")?.checked ?? false,
    trade_league: $("#st-trade-league")?.value.trim() || "",
    trade_filters: Number($("#st-trade-filters")?.value ?? 2),
    trade_headroom: Number($("#st-trade-headroom")?.value ?? 10),
    map_enabled: $("#st-map-enabled")?.checked ?? false,
    map_mechanics: ($("#st-map-mechanics")?.value || "")
      .split(",")
      .map((x) => x.trim())
      .filter(Boolean),
  };
}

async function saveSettings() {
  const settings = collectSettings();
  try {
    await invoke("save_settings", { settings });
    const key = $("#st-ai-key");
    if (key) key.value = "";
    toast(t("set.saved"));
  } catch (err) {
    toast(String(err), true);
  }
}

// ---------------------------------------------------------------- actions
async function act(name, el) {
  switch (name) {
    case "campaign-done":
      await invoke("campaign_complete");
      break;
    case "campaign-prev":
      await invoke("campaign_step", { delta: -1 });
      break;
    case "campaign-next":
      await invoke("campaign_step", { delta: 1 });
      break;
    case "campaign-goto":
      await invoke("campaign_goto", { step: Number(el.dataset.step) });
      break;
    case "route-toggle":
      await invoke("save_settings", {
        settings: { ...baseSettings(), route: S.saved.route === "must" ? "all" : "must" },
      });
      break;
    case "manual-zone": {
      const zone = $("#manual-zone")?.value;
      if (zone) await invoke("campaign_manual_zone", { zone });
      break;
    }
    case "zone-map": {
      const zone = $("#map-zone")?.value;
      if (zone) {
        await invoke("zone_map", { zone });
        toast(t("camp.map_done"));
      }
      break;
    }
    case "profile-new":
      await invoke("profile_new", { lang: S.saved.lang });
      break;
    case "profile-select":
      await invoke("profile_select", { id: el.dataset.id });
      break;
    case "profile-delete":
      if (confirm(t("char.delete_confirm"))) await invoke("profile_delete", { id: profile().id });
      break;
    case "profile-save":
      await saveProfile();
      break;
    case "build-select":
      break;
    case "overlay-toggle":
      await invoke("overlay_toggle");
      break;
    case "shot-take": {
      const kind = $("#shot-kind")?.value || "other";
      try {
        await invoke("shot_capture", { kind });
        toast(t("common.ok"));
        if (section === "shots") await loadShots();
      } catch (err) {
        toast(`${t("shot.capture_failed")}: ${err}`, true);
      }
      break;
    }
    case "shots-reveal":
      try {
        await invoke("shots_reveal");
      } catch (err) {
        toast(String(err), true);
      }
      break;
    case "shot-open":
      await openShot(el.dataset.id);
      break;
    case "shot-save":
      if (activeShot) {
        await invoke("shot_update", {
          patch: { id: activeShot.id, kind: $("#ms-kind").value, note: $("#ms-note").value },
        });
        $("#modal").hidden = true;
        activeShot = null;
        await loadShots();
      }
      break;
    case "shot-delete":
      if (activeShot && confirm(t("shot.delete_confirm"))) {
        await invoke("shot_delete", { id: activeShot.id });
        $("#modal").hidden = true;
        activeShot = null;
        await loadShots();
      }
      break;
    case "shot-ai":
      await prepareAi();
      break;
    case "ai-run":
      await runAi(el.dataset.task || "analyse_item");
      break;
    case "ai-test": {
      const out = $("#ai-test-result");
      if (out) out.textContent = t("set.ai_testing");
      try {
        await invoke("save_settings", { settings: collectSettings({ keepKey: true }) });
        const text = await invoke("ai_test");
        if (out) out.textContent = `${t("common.ok")}: ${text}`;
      } catch (err) {
        if (out) out.textContent = `${t("ai.failed")}: ${err}`;
        toast(String(err), true);
      }
      break;
    }
    case "history-export":
      try {
        const path = await invoke("history_export");
        toast(`${t("hist.exported")}: ${path}`);
      } catch (err) {
        toast(String(err), true);
      }
      break;
    case "log-auto":
      try {
        const info = await invoke("log_auto");
        toast(info.found ? `${t("set.log_current")}: ${info.path}` : t("chip.log_none"), !info.found);
      } catch (err) {
        toast(String(err), true);
      }
      break;
    case "item-import": {
      try {
        const rec = await invoke("item_import");
        toast(`${t("items.imported")}: ${rec.parsed.name || rec.parsed.base}`);
        await loadItems();
      } catch (err) {
        toast(String(err), true);
      }
      break;
    }
    case "item-delete":
      await invoke("item_delete", { id: el.dataset.id });
      await loadItems();
      break;
    case "zone-todo-add": {
      const input = $("#zt-input");
      const text = input?.value.trim() || "";
      if (text) await invoke("zone_todo_add", { text });
      break;
    }
    case "zone-todo-toggle":
      await invoke("zone_todo_toggle", { id: el.dataset.id });
      break;
    case "zone-todo-remove":
      await invoke("zone_todo_remove", { id: el.dataset.id });
      break;
    case "go-campaign":
      go("campaign");
      break;
    case "trade-run":
      await runTradeSearch($("#trade-q")?.value || "");
      break;
    case "trade-invite":
    case "trade-trade":
    case "trade-reply": {
      const kind = el.dataset.act.replace("trade-", "");
      await invoke("trade_action", { kind });
      toast(t("trade.copied"));
      break;
    }
    case "trade-clear":
      await invoke("trade_request_clear");
      break;
    case "trade-recent": {
      const q = el.dataset.q || "";
      const input = $("#trade-q");
      if (input) input.value = q;
      await runTradeSearch(q);
      break;
    }
    case "trade-recent-clear":
      localStorage.removeItem(RECENT_KEY);
      renderRecent();
      break;
    case "rate-refresh":
      await loadRate();
      break;
    case "find-item": {
      const q = el.dataset.q || "";
      go("trade");
      const input = $("#trade-q");
      if (input) input.value = q;
      await runTradeSearch(q);
      break;
    }
    case "trade-copy":
      await invoke("copy_text", { text: el.dataset.text || "" });
      toast(t("trade.copied"));
      break;
    case "trade-open":
      await invoke("link_open", { url: el.dataset.url || "" });
      break;
    case "guide-import":
      openGuideImport();
      break;
    case "guide-run":
      await runGuideImport();
      break;
    case "guide-variants":
      await loadGuideVariants();
      break;
    case "guide-upgrades":
      await runUpgrades(el.dataset.guide);
      break;
    case "guide-refresh": {
      el.disabled = true;
      try {
        await invoke("guide_refresh", { guideId: el.dataset.guide });
        toast(t("guide.refreshed"));
        renderSection();
      } catch (e) {
        toast(`${t("guide.refresh_failed")}: ${String(e)}`, true);
      } finally {
        el.disabled = false;
      }
      break;
    }
    case "guide-owned":
      await invoke("guide_toggle_owned", { guideId: el.dataset.guide, key: el.dataset.key });
      break;
    case "guide-find": {
      const out = el.closest("tr")?.querySelector(".guide-out");
      if (out) out.textContent = t("guide.wait");
      el.disabled = true;
      try {
        const res = await invoke("trade_search_guide", { guideId: el.dataset.guide, slot: el.dataset.slot });
        if (!out) break;
        if (!res.listings.length) {
          out.textContent = t("guide.empty");
        } else {
          out.innerHTML = res.listings
            .slice(0, 3)
            .map(
              (l) =>
                `<div><span class="dot-online ${l.online ? "on" : "off"}"></span> ${esc(String(l.price))} ${esc(l.currency)} · ${esc(l.account)}${l.whisper ? ` <button class="link" data-act="trade-copy" data-text="${esc(l.whisper)}">whisper</button>` : ""}</div>`
            )
            .join("");
        }
      } catch (e) {
        if (out) out.innerHTML = `<span class="bad-text">${esc(String(e))}</span>`;
      } finally {
        el.disabled = false;
      }
      break;
    }
    case "pob-import":
      openPobImport();
      break;
    case "pob-run":
      await runPobImport();
      break;
    case "item-price": {
      const box = el.closest(".item")?.querySelector(".price-slot");
      if (box) {
        box.hidden = false;
        box.innerHTML = `<div class="small">${esc(t("items.price_wait"))}</div>`;
      }
      el.disabled = true;
      try {
        const p = await invoke("trade_price", { id: el.dataset.id });
        const money = (n) => (Number.isInteger(n) ? String(n) : n.toFixed(1));
        if (box) {
          const prev = p.previous;
          const was = prev && prev.currency === p.currency ? ` · ${esc(t("items.was"))} ${money(prev.min)} → ${money(p.min)}` : "";
          const line =
            p.total === 0
              ? esc(p.note || t("items.price_failed"))
              : `${esc(t("items.price_min"))} <b>${money(p.min)} ${esc(p.currency)}</b> · ${esc(t("items.price_median"))} <b>${money(p.median)} ${esc(p.currency)}</b> · ${p.total} ${esc(t("items.price_lots"))}${was}`;
          box.innerHTML = `<div class="small">${line}</div>
            <div class="small">${esc(p.league)}${p.note ? " · " + esc(p.note) : ""} · <button class="link" data-act="link" data-url="${esc(p.trade_url)}">${esc(t("items.price_open"))}</button></div>`;
        }
      } catch (err) {
        if (box) box.innerHTML = `<div class="small bad-text">${esc(t("items.price_failed"))}: ${esc(String(err))}</div>`;
      } finally {
        el.disabled = false;
      }
      break;
    }
    case "item-ai": {
      const box = el.closest(".item")?.querySelector(".ai-slot");
      if (box) {
        box.hidden = false;
        box.innerHTML = `<div class="small">${esc(t("ai.waiting"))}</div>`;
      }
      el.disabled = true;
      try {
        await invoke("item_ai", { id: el.dataset.id, question: "" });
        await loadItems();
      } catch (err) {
        if (box) box.innerHTML = `<div class="small bad-text">${esc(t("ai.failed"))}: ${esc(String(err))}</div>`;
        toast(String(err), true);
      } finally {
        el.disabled = false;
      }
      break;
    }
    case "regex-add":
      $("#regex-rows")?.insertAdjacentHTML("beforeend", regexRow());
      break;
    case "pick-log": {
      const path = await invoke("pick_log_file");
      const field = $("#st-log-path");
      if (path && field) {
        field.value = path;
        const mode = $("#st-log-mode");
        if (mode) mode.value = "manual";
        toast(path);
      }
      break;
    }
    case "pick-shots-dir": {
      const dir = await invoke("pick_folder");
      const field = $("#st-shots-dir");
      if (dir && field) {
        field.value = dir;
        toast(dir);
      }
      break;
    }
    case "log-check": {
      const path = $("#st-log-path")?.value.trim();
      if (!path) {
        toast(t("set.log_path"), true);
        break;
      }
      try {
        const res = await invoke("log_check", { path });
        const out = $("#log-check-result");
        const text = `${res.message}${res.zone ? ` · ${t("home.zone")}: ${res.zone}` : ""}${res.level ? ` · ${t("char.level")}: ${res.level}` : ""}`;
        if (out) out.textContent = text;
        toast(text, !res.ok);
      } catch (err) {
        toast(String(err), true);
      }
      break;
    }
    case "save-settings":
      await saveSettings();
      break;
    case "reset-pos":
      await invoke("save_overlay", { overlay: { ...S.saved.overlay, x: 24, y: 160 } });
      toast(t("set.reset_pos_done"));
      break;
    case "alias-add":
      $("#alias-rows")?.insertAdjacentHTML("beforeend", aliasRow());
      break;
    case "ai-clear-key":
      try {
        await invoke("save_settings", { settings: { ...baseSettings(), ai_clear_key: true } });
        toast(t("common.ok"));
      } catch (err) {
        toast(String(err), true);
      }
      break;
    case "diag-refresh":
      await refreshDiagnostics();
      break;
    case "diag-copy": {
      const text = $("#diag-box")?.textContent || "";
      await invoke("copy_text", { text });
      toast(t("common.copied"));
      break;
    }
    case "data-dir":
      await invoke("reveal_data_dir").catch((e) => toast(String(e), true));
      break;
    case "link":
      await invoke("link_open", { url: el.dataset.url }).catch((e) => toast(String(e), true));
      break;
    case "go-character":
      go("character");
      break;
    case "go-build":
      go("build");
      break;
    case "go-campaign":
      go("campaign");
      break;
    case "modal-close":
      $("#modal").hidden = true;
      activeShot = null;
      break;
    case "quit":
      await invoke("quit_app");
      break;
    default:
      break;
  }
}

// ---------------------------------------------------------------- events
document.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && e.target?.id === "zt-input") {
    e.preventDefault();
    $("#content [data-act='zone-todo-add']")?.click();
  }
});

document.addEventListener("click", (e) => {
  const nav = e.target.closest("[data-nav]");
  if (nav) return go(nav.dataset.nav);
  const el = e.target.closest("[data-act]");
  if (el) {
    e.preventDefault();
    act(el.dataset.act, el).catch((err) => toast(String(err), true));
  }
});

document.addEventListener("change", async (e) => {
  if (e.target.id === "build-select") {
    await invoke("build_select", { id: e.target.value });
  }
  if (e.target.matches('input[data-item][type="checkbox"]')) {
    await invoke("build_toggle", { item: e.target.dataset.item });
  }
});

document.addEventListener("keydown", (e) => {
  if (capturing) {
    e.preventDefault();
    if (["Control", "Alt", "Shift", "Meta"].includes(e.key)) return;
    if (e.key === "Escape") {
      capturing.classList.remove("capturing");
      capturing.textContent = prettyHotkey(capturing.dataset.value);
      capturing = null;
      return;
    }
    if (e.key === "Delete" || e.key === "Backspace") {
      capturing.dataset.value = "";
      capturing.textContent = t("common.none");
      capturing.classList.remove("capturing");
      capturing = null;
      return;
    }
    const mods = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift"].filter(Boolean);
    const combo = [...mods, e.code].join("+");
    capturing.dataset.value = combo;
    capturing.textContent = prettyHotkey(combo);
    capturing.classList.remove("capturing");
    capturing = null;
    return;
  }
  if (e.key === "Escape" && !$("#modal").hidden) {
    $("#modal").hidden = true;
    activeShot = null;
    return;
  }
  if (e.key === "F5") {
    e.preventDefault();
    renderSection();
  }
});

$("#modal").addEventListener("click", (e) => {
  if (e.target.id === "modal") {
    $("#modal").hidden = true;
    activeShot = null;
  }
});

// Re-render after the user stops typing, so a log event does not eat input.
let pendingRender = false;
document.addEventListener("focusout", () => {
  if (!pendingRender) return;
  const a = document.activeElement;
  if (a && /^(INPUT|TEXTAREA|SELECT)$/.test(a.tagName)) return;
  pendingRender = false;
  setTimeout(() => {
    if (!/^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement?.tagName || "")) renderSection();
  }, 0);
});

async function boot() {
  const boot = await invoke("bootstrap");
  S = boot.state;
  CAMP = boot.campaign;
  BUILDS = boot.builds;
  VERSION = boot.version;
  setLang(S.saved.lang);
  document.querySelectorAll("[data-i18n]").forEach((el) => (el.textContent = t(el.dataset.i18n)));
  renderNav();
  await loadHistory(200);
  go((location.hash || "#home").slice(1));
  await listen("guide-progress", (e) => {
  const el = $("#guide-stage");
  if (!el) return;
  const stage = e.payload?.stage || "";
  el.hidden = false;
  el.textContent = t("build.stage_" + stage) || stage;
});

await listen("state", async (e) => {
    S = e.payload;
    // A build imported (or removed) elsewhere must show up here too.
    const ids = BUILDS.map((b) => b.id).sort().join(",");
    if ((S.build_ids || []).slice().sort().join(",") !== ids) {
      try {
        BUILDS = await invoke("builds_list");
      } catch {}
    }
    if (S.saved.lang !== LANG) {
      setLang(S.saved.lang);
      renderNav();
    }
    if (section === "home" || section === "history") await loadHistory(section === "home" ? 200 : 2000);
    renderChips();
    const a = document.activeElement;
    if (a && $("#content").contains(a) && /^(INPUT|TEXTAREA|SELECT)$/.test(a.tagName)) pendingRender = true;
    else renderSection();
  });
  setInterval(renderChips, 5000);
}

boot().catch((err) => {
  document.body.innerHTML = `<pre style="color:#e8685a;padding:20px;font:13px monospace">${esc(err?.stack || err)}</pre>`;
});
