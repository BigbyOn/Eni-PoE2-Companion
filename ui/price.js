const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (sel) => document.querySelector(sel);
const money = (n) => (Number.isInteger(n) ? String(n) : Number(n).toFixed(1));

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
  $("#p-value").textContent = `${money(q.min)}–${money(q.median)} ${q.currency}`;
  $("#p-line").textContent = `${q.total} лотов${q.divine != null ? ` · ≈ ${money(q.divine)} divine` : ""}`;
  $("#p-note").textContent = q.note || "";
}

(async () => {
  try {
    const boot = await invoke("bootstrap");
    render(boot.state.quick_price);
  } catch {}
  await listen("state", (event) => render(event.payload.quick_price));
})();
