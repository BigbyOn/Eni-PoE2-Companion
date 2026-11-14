const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (sel) => document.querySelector(sel);
const money = (n) => {
  const v = Number(n);
  if (!Number.isFinite(v)) return "—";
  if (Number.isInteger(v)) return String(v);
  if (Math.abs(v) >= 1) return v.toFixed(1);
  if (Math.abs(v) >= 0.01) return v.toFixed(3);
  return v.toFixed(4);
};
const CURRENCY_ICONS = {
  divine: "icons/currency/divine.png",
  exalted: "icons/currency/exalted.png",
  chaos: "icons/currency/chaos.png",
  regal: "icons/currency/regal.png",
  alch: "icons/currency/alch.png",
  vaal: "icons/currency/vaal.png",
  annul: "icons/currency/annul.png",
  mirror: "icons/currency/mirror.png",
};

function currencyIcon(currency) {
  const key = String(currency || "").toLowerCase().replace(/ orb$/, "").trim();
  const src = CURRENCY_ICONS[key];
  return src ? `<img class="cur-icon" src="${src}" alt="">` : "";
}
let STATE = null;

function applyScale() {
  const scale = Number(STATE?.saved?.overlay?.price_scale ?? 100);
  document.documentElement.style.zoom = String(Math.min(150, Math.max(70, scale)) / 100);
}

function render(q) {
  const box = $("#price");
  if (!q) {
    box.hidden = true;
    return;
  }
  box.hidden = false;
  $("#p-name").textContent = q.name || "Цена";
  if (q.error) {
    $("#p-value").textContent = "—";
    $("#p-line").textContent = q.error;
    $("#p-note").textContent = "";
    return;
  }
  if (!q.total) {
    $("#p-value").textContent = "…";
    $("#p-line").textContent = q.note || "Смотрю цены…";
    $("#p-note").textContent = "";
    return;
  }
  $("#p-value").innerHTML = `${currencyIcon(q.currency)}${money(q.min)}–${money(q.median)} ${q.currency}`;
  $("#p-line").textContent = `${q.total} ${q.total === 1 ? "лот" : "лотов"}${q.divine != null ? ` · ≈ ${money(q.divine)} divine` : ""}${q.matched ? ` · модов: ${q.matched}` : ""}`;
  $("#p-note").textContent = q.note || "";
}

(async () => {
  try {
    const boot = await invoke("bootstrap");
    STATE = boot.state;
    applyScale();
    render(boot.state.quick_price);
  } catch {}
  await listen("state", (event) => {
    STATE = event.payload;
    applyScale();
    render(event.payload.quick_price);
  });
})();
