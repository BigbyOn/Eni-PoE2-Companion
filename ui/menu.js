const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);
const form = $("settings");
// Two windows load this page: "menu" (settings, focusable) and "regex" (the list,
// never focused, so the game keeps the keyboard).
const view = window.__TAURI__.window.getCurrentWindow().label === "regex" ? "regex" : "settings";
let state, capturing = null;

const L = (de, en) => (state?.lang === "en" ? en : de);
const esc = (s) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
// "Ctrl+Alt+KeyR" -> "Ctrl+Alt+R"
const pretty = (k) => k.replace(/Key([A-Z])/g, "$1").replace(/Digit(\d)/g, "$1").replace("ArrowRight", "→").replace("ArrowLeft", "←").replace("ArrowUp", "↑").replace("ArrowDown", "↓");

function translate() {
  document.querySelectorAll("[data-de]").forEach((el) => (el.textContent = L(el.dataset.de, el.dataset.en)));
}

const LOOK = ["full_size", "mini_size", "opacity"];
const look = () => Object.fromEntries(LOOK.map((k) => [k, +$(k).value]));
function showLook() {
  LOOK.forEach((k) => (document.querySelector(`output[for=${k}]`).textContent = `${$(k).value} %`));
}
// Live preview in the panel; "Abbrechen" or closing drops it again.
LOOK.forEach((k) =>
  $(k).addEventListener("input", () => {
    showLook();
    invoke("preview_look", { look: look() });
  })
);

function regexRow(r = { name: "", value: "" }) {
  const row = document.createElement("div");
  row.className = "row";
  row.innerHTML = `<input type="text" class="name" placeholder="${L("Name", "Name")}"><input type="text" class="value" placeholder="Regex"><button type="button">&#10005;</button>`;
  row.querySelector(".name").value = r.name;
  row.querySelector(".value").value = r.value;
  row.querySelector("button").onclick = () => row.remove();
  $("rows").append(row);
}

async function showSettings() {
  form.elements.lang.value = state.lang;
  form.elements.route.value = state.route;
  LOOK.forEach((k) => ($(k).value = state.look[k]));
  $("guide_on").checked = state.guide_on;
  showLook();
  document.querySelectorAll(".hk").forEach((b) => {
    b.dataset.value = state.hotkeys[b.dataset.k];
    b.textContent = pretty(b.dataset.value);
  });
  $("rows").innerHTML = "";
  state.regexes.forEach(regexRow);
  const meta = JSON.parse(await invoke("guide")).meta;
  $("guideinfo").textContent = L(`Route von poe2speedrun.malura.de, Patch ${meta.patch}, Stand ${meta.updated}.`, `Route from poe2speedrun.malura.de, patch ${meta.patch}, updated ${meta.updated}.`);
  $("logpath").textContent = state.log_path ? `Client.txt: ${state.log_path}` : L("Client.txt nicht gefunden.", "Client.txt not found.");
  $("error").textContent = state.hotkey_error || "";
}

function showRegex() {
  const list = state.regexes;
  $("picks").innerHTML = list
    .map((r, i) => `<button type="button" class="pick" data-i="${i}"><span class="n">${i < 9 ? i + 1 : ""}</span><b>${esc(r.name || "Regex")}</b><code>${esc(r.value)}</code></button>`)
    .join("");
  $("empty").hidden = list.length > 0;
  $("empty").textContent = L(`Noch keine Regexe. In den Einstellungen (${pretty(state.hotkeys.settings)}) anlegen.`, `No regexes yet. Add some in the settings (${pretty(state.hotkeys.settings)}).`);
}

async function copy(i) {
  const btn = document.querySelector(`.pick[data-i="${i}"]`);
  if (!btn) return;
  btn.classList.add("copied");
  await invoke("copy_regex", { value: state.regexes[i].value });
  btn.classList.remove("copied");
}

async function open(v) {
  capturing = null;
  state = await invoke("snapshot");
  translate();
  $("title").textContent = v === "settings" ? L("Einstellungen", "Settings") : L("Regex kopieren", "Copy regex");
  form.hidden = v !== "settings";
  $("regex").hidden = v !== "regex";
  if (v === "settings") await showSettings();
  else showRegex();
  window.focus();
}

$("picks").addEventListener("click", (e) => {
  const b = e.target.closest(".pick");
  if (b) copy(+b.dataset.i);
});

document.querySelectorAll(".hk").forEach((b) =>
  b.addEventListener("click", () => {
    capturing?.classList.remove("capturing");
    capturing = b;
    b.classList.add("capturing");
    b.textContent = L("Tasten drücken ...", "Press keys ...");
  })
);

function stopCapture(value) {
  if (value !== undefined) capturing.dataset.value = value;
  capturing.textContent = pretty(capturing.dataset.value);
  capturing.classList.remove("capturing");
  capturing = null;
}

addEventListener("keydown", (e) => {
  if (capturing) {
    e.preventDefault();
    if (["Control", "Alt", "Shift", "Meta"].includes(e.key)) return;
    if (e.key === "Escape") return stopCapture();
    if (e.key === "Delete" || e.key === "Backspace") return stopCapture("");
    const mods = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift"].filter(Boolean);
    return stopCapture([...mods, e.code].join("+"));
  }
  if (e.key === "Escape") return invoke("close");
  if (view === "regex" && /^[1-9]$/.test(e.key)) copy(+e.key - 1);
});

// Mobalytics' copy button puts the regex only into the HTML clipboard format,
// the plain text is empty. Take the text out of the HTML then.
$("rows").addEventListener("paste", (e) => {
  const data = e.clipboardData;
  if (!data || data.getData("text/plain") || !data.getData("text/html")) return;
  e.preventDefault();
  const text = new DOMParser().parseFromString(data.getData("text/html"), "text/html").body.textContent.trim();
  e.target.setRangeText(text, e.target.selectionStart, e.target.selectionEnd, "end");
});

$("add").onclick = () => regexRow();
$("reload").onclick = async () => {
  await invoke("reload_guide");
  setTimeout(showSettings, 1500);
};
$("cancel").onclick = () => invoke("close");
$("x").onclick = () => invoke("close");

form.addEventListener("submit", async (e) => {
  e.preventDefault();
  const hotkeys = {};
  document.querySelectorAll(".hk").forEach((b) => (hotkeys[b.dataset.k] = b.dataset.value));
  const regexes = [...document.querySelectorAll("#rows .row")].map((r) => ({
    name: r.querySelector(".name").value.trim(),
    value: r.querySelector(".value").value,
  }));
  try {
    await invoke("save_settings", { settings: { lang: form.elements.lang.value, route: form.elements.route.value, hotkeys, regexes, look: look(), guide_on: $("guide_on").checked } });
    invoke("close");
  } catch (err) {
    $("error").textContent = err;
  }
});

listen("menu-view", (e) => {
  if ((e.payload === "regex") === (view === "regex")) open(view);
});
