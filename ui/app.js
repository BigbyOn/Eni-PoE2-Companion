const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);
const panel = $("panel");
let guide, state;

const esc = (s) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
const md = (s) => esc(s).replace(/\*\*(.+?)\*\*/g, "<b>$1</b>");
const t = (v) => (v ? v[state.lang] || v.de : "");
// Goal line: task in white, **names** in gold, everything after "->" is the reward (mod blue).
function goal(s) {
  const [task, ...reward] = t(s.text).split(" -> ");
  return `<span class="task">${md(task)}</span>` + (reward.length ? ` <span class="reward">-&gt; ${md(reward.join(" -> "))}</span>` : "");
}

// XP penalty window: 3 levels + 1 per 16 character levels.
function levelWarn(char, area) {
  return char && area && Math.abs(area - char) > 3 + Math.floor(char / 16);
}

// Same matching as src/guide.rs: "A -> B" and "A / B" mean either, a leading "The" is ignored.
const norm = (z) => z.trim().toLowerCase().replace(/^the /, "");
const zonesOf = (z) => z.split("->").flatMap((p) => p.split("/")).map(norm).filter(Boolean);

// The step's zone if the player is somewhere else (town, hideout, wrong zone), else "".
function goTo(s) {
  if (!state.zone || zonesOf(s.zone).includes(norm(state.zone))) return "";
  return s.zone.split("->")[0].split("/")[0].trim();
}

// Route mode like the presets on poe2speedrun (same rule as src/guide.rs).
function shown(s) {
  if (state.route === "must") return s.tags.includes("must");
  if (state.route === "buffs") return s.tags.includes("must") || s.tags.includes("buff");
  return true;
}
const after = (i) => guide.steps.slice(i + 1).filter(shown);

// First later shown step in a different zone: where to go once this step is done.
function nextZone(i) {
  const here = zonesOf(guide.steps[i].zone);
  const n = after(i).find((n) => !zonesOf(n.zone).some((z) => here.includes(z)));
  return n ? n.zone.split("->")[0].split("/")[0].trim() : "";
}

// Text size per mode (CSS zoom) and background opacity, from the settings.
function applyLook(look) {
  panel.style.zoom = (state.mini ? look.mini_size : look.full_size) / 100;
  panel.style.setProperty("--panel", `rgba(12,11,9,${look.opacity / 100})`);
  panel.classList.toggle("glass", look.opacity < 50);
  requestAnimationFrame(reportSize);
}

function render() {
  const i = state.step, s = guide.steps[i];
  const act = guide.acts[s.act];
  document.documentElement.lang = state.lang;
  panel.classList.toggle("mini", state.mini);
  panel.hidden = false;
  applyLook(state.look);

  const view = guide.steps.filter(shown);
  $("act").textContent = `${t(act.short)} · ${view.indexOf(s) + 1}/${view.length}`;
  const lv = $("levels");
  lv.textContent = state.char_level ? `Lvl ${state.char_level}` + (state.area_level ? ` · ${state.lang === "de" ? "Gebiet" : "Area"} ${state.area_level}` : "") : "";
  lv.classList.toggle("warn", levelWarn(state.char_level, state.area_level));

  const cur = $("current");
  cur.className = s.tags.join(" ");
  const go = goTo(s);
  const goLabel = state.lang === "de" ? "Gehe nach" : "Go to";
  const next = nextZone(i);
  const then = next ? `<span class="then">${state.lang === "de" ? "danach" : "then"}: <span class="dest">${esc(next)}</span></span>` : "";
  if (state.mini) {
    // One line. Elsewhere: where to go and what to do there. In the zone: the task, then where to go next.
    cur.innerHTML = go
      ? `<span class="go">${goLabel} <span class="dest">${esc(go)}</span>:</span> <span class="goal">${goal(s)}</span>`
      : `<span class="goal">${goal(s)}</span>${then}`;
    $("upcoming").innerHTML = "";
    $("nolog").hidden = true;
    return;
  }
  const opt = s.tags.includes("must") ? "" : `<span class="optional">optional</span>`;
  cur.innerHTML =
    (go ? `<div class="go">${goLabel} <span class="dest">${esc(go)}</span><span class="here">${esc(state.zone)}</span></div>` : "") +
    `<div class="zone">${esc(s.zone)}${s.level ? `<span class="lvl">Lvl ${esc(s.level)}</span>` : ""}${opt}</div>` +
    `<div class="goal">${goal(s)}</div>` +
    (then ? `<div>${then}</div>` : "") +
    (s.hint ? `<div class="hint">${md(t(s.hint))}</div>` : "") +
    (s.notes.length ? `<ul class="notes">${s.notes.map((n) => `<li>${md(t(n))}</li>`).join("")}</ul>` : "");

  $("upcoming").innerHTML = after(i)
    .slice(0, 2)
    .map((n) => `<li><span class="z">${esc(n.zone)}</span>${md(t(n.text))}</li>`)
    .join("");
  $("nolog").hidden = state.log_found;
  $("hkerr").hidden = !state.hotkey_error;
  $("hkerr").textContent = state.hotkey_error || "";
}

// The window is exactly the panel: Rust sizes it to what we render here.
function reportSize() {
  const r = panel.getBoundingClientRect();
  invoke("panel_size", { w: r.width, h: r.height });
}
new ResizeObserver(reportSize).observe(panel);

$("prev").onclick = () => invoke("step", { delta: -1 });
$("next").onclick = () => invoke("step", { delta: 1 });
$("mini").onclick = () => invoke("toggle_mini");
$("off").onclick = () => invoke("guide_off");

// Drag by the header (mini mode: by the whole line, a plain click opens the full panel).
// Rust moves the window from the real cursor position; pointer capture keeps
// the events coming even if the mouse outruns the window.
panel.addEventListener("pointerdown", (e) => {
  if (e.button !== 0 || e.target.closest("button") || (!state.mini && !e.target.closest("#bar"))) return;
  panel.setPointerCapture(e.pointerId);
  const sx = e.screenX, sy = e.screenY;
  let moved = false;
  invoke("drag", { phase: "start" });
  const move = (m) => {
    moved ||= Math.abs(m.screenX - sx) + Math.abs(m.screenY - sy) > 3;
    if (moved) invoke("drag", { phase: "move" });
  };
  const up = () => {
    panel.removeEventListener("pointermove", move);
    panel.removeEventListener("pointerup", up);
    invoke("drag", { phase: "end" });
    if (!moved && state.mini) invoke("toggle_mini");
  };
  panel.addEventListener("pointermove", move);
  panel.addEventListener("pointerup", up);
});

(async () => {
  guide = JSON.parse(await invoke("guide"));
  state = await invoke("snapshot");
  render();
  listen("look", (e) => applyLook(e.payload));
  listen("state", (e) => {
    state = e.payload;
    render();
  });
  // A newer route arrived from poe2speedrun.
  listen("guide", async () => {
    guide = JSON.parse(await invoke("guide"));
    render();
  });
})();
