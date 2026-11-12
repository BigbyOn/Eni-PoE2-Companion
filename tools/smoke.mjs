// Smoke test against the running app through the WebView2 devtools protocol.
// Usage: node tools/smoke.mjs <tempdir>
// Starts nothing itself: run the app with
//   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333
import { readFileSync, existsSync, statSync, writeFileSync, mkdirSync, readdirSync } from "node:fs";
import { join } from "node:path";

const PORT = Number(process.env.ENI_DEVTOOLS_PORT || 9333);
const tmp = process.argv[2];
if (!tmp) throw new Error("pass a temp dir");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const log = (...a) => console.log(...a);

async function listTargets() {
  const hosts = [`[::1]`, "127.0.0.1", "localhost"];
  for (let i = 0; i < 60; i++) {
    for (const host of hosts) {
      try {
        const res = await fetch(`http://${host}:${PORT}/json/list`);
        const list = await res.json();
        if (list.length >= 2) {
          log("devtools via", host);
          return list;
        }
      } catch {}
    }
    await sleep(500);
  }
  throw new Error("devtools endpoint not reachable");
}

class CDP {
  constructor(url, label) {
    this.url = url;
    this.label = label;
    this.id = 0;
    this.pending = new Map();
    this.errors = [];
    this.logs = [];
  }
  async open() {
    this.ws = new WebSocket(this.url);
    await new Promise((res, rej) => {
      this.ws.onopen = res;
      this.ws.onerror = (e) => rej(new Error("ws error " + this.label));
    });
    this.ws.onmessage = (ev) => {
      const msg = JSON.parse(ev.data);
      if (msg.id) {
        const p = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        if (!p) return;
        msg.error ? p.rej(new Error(JSON.stringify(msg.error))) : p.res(msg.result);
        return;
      }
      if (msg.method === "Runtime.exceptionThrown") {
        this.errors.push(msg.params.exceptionDetails?.exception?.description || msg.params.exceptionDetails?.text || "exception");
      }
      if (msg.method === "Log.entryAdded" && msg.params.entry.level === "error") this.logs.push(msg.params.entry.text);
      if (msg.method === "Runtime.consoleAPICalled" && msg.params.type === "error") {
        this.logs.push(msg.params.args.map((a) => a.value ?? a.description ?? "").join(" "));
      }
    };
  }
  send(method, params = {}) {
    const id = ++this.id;
    return new Promise((res, rej) => {
      this.pending.set(id, { res, rej });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }
  async eval(expression, awaitPromise = true) {
    const r = await this.send("Runtime.evaluate", { expression, awaitPromise, returnByValue: true, userGesture: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || JSON.stringify(r.exceptionDetails));
    return r.result.value;
  }
  close() {
    try { this.ws.close(); } catch {}
  }
}

async function waitFor(cdp, expr, ms = 8000) {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    if (await cdp.eval(`Boolean(${expr})`)) return true;
    await sleep(150);
  }
  return false;
}

const results = [];
function check(name, ok, extra = "") {
  results.push({ name, ok, extra });
  log(`${ok ? "PASS" : "FAIL"}  ${name}${extra ? " — " + extra : ""}`);
}

const targets = await listTargets();
log("targets:", targets.map((t) => `${t.title || "?"} (${t.url})`).join(", "));
const overlayTarget = targets.find((t) => t.url.includes("overlay.html"));
const mainTarget = targets.find((t) => t !== overlayTarget && t.type === "page");
if (!mainTarget) throw new Error("main window target not found");

const main = new CDP(mainTarget.webSocketDebuggerUrl, "main");
await main.open();
await main.send("Runtime.enable");
await main.send("Log.enable");
await sleep(1200);

// 1. shell rendered
const nav = await main.eval(`document.querySelectorAll('#nav-items button').length`);
check("main: 7 sections in the sidebar", nav === 7, `got ${nav}`);
const title = await main.eval(`document.querySelector('#page-title').textContent`);
check("main: page title rendered", title.length > 0, title);
const boot = await main.eval(`JSON.stringify({ lang: S.saved.lang, steps: CAMP.steps.length, acts: CAMP.acts.length, builds: BUILDS.length, version: VERSION })`);
check("main: bootstrap data", JSON.parse(boot).steps === 115 && JSON.parse(boot).builds >= 1, boot);

// 2. every section renders without throwing
for (const id of ["home", "character", "build", "campaign", "history", "shots", "settings"]) {
  const len = await main.eval(`(async () => { go('${id}'); await new Promise(r => setTimeout(r, 250)); return document.querySelector('#content').innerText.length; })()`);
  check(`main: section "${id}" renders`, len > 40, `${len} chars`);
}

// 3. language switch through the real command path
const langRes = await main.eval(`(async () => {
  const s = JSON.parse(JSON.stringify(S.saved));
  s.lang = 'en';
  await window.__TAURI__.core.invoke('save_settings', { settings: {
    lang: s.lang, route: s.route, hotkeys: s.hotkeys, log: s.log, overlay: s.overlay,
    shots_dir: s.shots_dir, zone_aliases: s.zone_aliases,
    ai_provider: s.ai.provider, ai_model: s.ai.model, ai_reasoning: s.ai.reasoning || 'auto', ai_enabled: s.ai.enabled, ai_key: '', ai_clear_key: false } });
  await new Promise(r => setTimeout(r, 400));
  return document.documentElement.lang + '|' + document.querySelector('#nav-items button').textContent.trim();
})()`);
check("main: English switch", langRes.startsWith("en|Home"), langRes);

// back to Russian
await main.eval(`(async () => {
  const s = JSON.parse(JSON.stringify(S.saved));
  s.lang = 'ru';
  await window.__TAURI__.core.invoke('save_settings', { settings: {
    lang: 'ru', route: s.route, hotkeys: s.hotkeys, log: s.log, overlay: s.overlay,
    shots_dir: s.shots_dir, zone_aliases: s.zone_aliases,
    ai_provider: s.ai.provider, ai_model: s.ai.model, ai_reasoning: s.ai.reasoning || 'auto', ai_enabled: s.ai.enabled, ai_key: '', ai_clear_key: false } });
  await new Promise(r => setTimeout(r, 300));
  return document.documentElement.lang;
})()`);

// 4. point the app at the synthetic log and let it follow real writes
const logPath = join(tmp, "Client.txt");
const shotsDir = join(tmp, "shots");
mkdirSync(shotsDir, { recursive: true });
await main.eval(`(async () => {
  const s = JSON.parse(JSON.stringify(S.saved));
  await window.__TAURI__.core.invoke('save_settings', { settings: {
    lang: s.lang, route: s.route, hotkeys: s.hotkeys,
    log: { mode: 'manual', path: ${JSON.stringify(logPath)} },
    overlay: s.overlay, shots_dir: ${JSON.stringify(shotsDir)}, zone_aliases: s.zone_aliases,
    ai_provider: s.ai.provider, ai_model: s.ai.model, ai_reasoning: s.ai.reasoning || 'auto', ai_enabled: s.ai.enabled, ai_key: '', ai_clear_key: false } });
})()`);
await waitFor(main, "S.log_found && String(S.log_path).includes('_smoke') && S.saved.profiles[0].level === 6");
const saveLog = await main.eval(`JSON.stringify({ found: S.log_found, path: S.log_path, zone: S.saved.profiles[0].zone, level: S.saved.profiles[0].level })`);
const st = JSON.parse(saveLog);
check("log: manual path accepted and followed", st.found === true && String(st.path).includes("Client.txt"), saveLog);
check("log: catch-up restored zone", st.zone === "The Grim Tangle", st.zone);
check("log: catch-up restored level", st.level === 6, String(st.level));

// 5. appended lines move the profile, the route and the history
writeFileSync(logPath, "", { flag: "a" });
const w = (s) => writeFileSync(logPath, s, { flag: "a" });
w("[INFO Client 1] [SCENE] Set Source [Cemetery of the Eternals]\n");
w("[INFO Client 1] : Eni (Ranger) is now level 7\n");
w("[INFO Client 1] : Eni has received +10% to [Resistances|Cold Resistance].\n");
await waitFor(main, "S.saved.profiles[0].level === 7 && S.saved.profiles[0].zone === 'Cemetery of the Eternals'");
const a = JSON.parse(await main.eval(`(async () => JSON.stringify({
  zone: S.saved.profiles[0].zone,
  level: S.saved.profiles[0].level,
  step: S.saved.profiles[0].step,
  goal: CAMP.steps[S.saved.profiles[0].step].goal.ru,
  zone_unknown: S.zone_unknown,
  rewards: S.saved.profiles[0].rewards.map(r => r.kind),
  history: (await window.__TAURI__.core.invoke('history_list', { filter: { limit: 20 } })).length,
}))()`));
check("log: zone change applied", a.zone === "Cemetery of the Eternals", a.zone);
check("log: route advanced into act 1", a.step >= 8 && a.step <= 10, `step ${a.step}: ${a.goal}`);
check("log: level-up applied and name bound", a.level === 7, String(a.level));
check("log: reward recorded", a.rewards.includes("cold_resistance"), a.rewards.join(","));
check("log: history written", a.history >= 3, String(a.history));

// 6. Russian client lines, formats taken from a real Russian log
w("[INFO Client 1] : Eni (Охотница) достигает 8 уровня\n");
w("[INFO Client 1] : Игрок Eni получил +10% к сопротивлению [Resistances|холоду].\n");
w("[INFO Client 1] : Вы получили 2 Passive Skill Points.\n");
await waitFor(main, "S.saved.profiles[0].level === 8 && S.saved.profiles[0].rewards.some(r => r.kind === 'passive_points')");
const ru = JSON.parse(await main.eval(`(async () => JSON.stringify({
  level: S.saved.profiles[0].level,
  class: S.saved.profiles[0].class,
  rewards: S.saved.profiles[0].rewards.map(r => r.kind),
}))()`));
check("ru log: level-up", ru.level === 8, String(ru.level));
check("ru log: class canonicalized", ru.class === "Huntress", ru.class);
check("ru log: reward parsed", ru.rewards.includes("passive_points"), ru.rewards.join(","));

// 7. screenshot capture of the fake game window
const shot = JSON.parse(await main.eval(`(async () => {
  const res = await window.__TAURI__.core.invoke('shot_capture', { kind: 'item' });
  return JSON.stringify(res);
})()`));
const shotPath = join(shotsDir, shot.file);
const shotOk = existsSync(shotPath) && statSync(shotPath).size > 2000;
check("screenshot: file written", shotOk, `${shot.file} ${shot.w}×${shot.h} ${existsSync(shotPath) ? statSync(shotPath).size : 0}b`);
const rect = await main.eval(`(async () => {
  const d = await window.__TAURI__.core.invoke('diagnostics');
  const m = d.match(/game client area: (-?\\d+),(-?\\d+) (\\d+)x(\\d+)/);
  return m ? [Number(m[3]), Number(m[4])] : [0, 0];
})()`);
check("screenshot: size equals the game client area", rect[0] === shot.w && rect[1] === shot.h, `${shot.w}×${shot.h} vs client ${rect.join("×")}`);
const thumbOk = existsSync(join(shotsDir, shot.thumb));
check("screenshot: thumbnail written", thumbOk, shot.thumb);

// 7b. AI UI is present and a disabled AI fails with a clear message
const aiUi = JSON.parse(await main.eval(`(async () => {
  go('settings');
  await new Promise(r => setTimeout(r, 300));
  return JSON.stringify({
    provider: Boolean(document.querySelector('#st-ai-provider')),
    model: Boolean(document.querySelector('#st-ai-model')),
    reasoning: Boolean(document.querySelector('#st-ai-reasoning')),
    reasoningOptions: document.querySelector('#st-ai-reasoning') ? [...document.querySelector('#st-ai-reasoning').options].map(o => o.value) : [],
    key: Boolean(document.querySelector('#st-ai-key')),
    test: Boolean(document.querySelector('[data-act="ai-test"]')),
    visionHint: document.querySelector('#ai-vision-hint')?.textContent || '',
  });
})()`));
check("ai ui: provider/model/reasoning/key/test present", aiUi.provider && aiUi.model && aiUi.reasoning && aiUi.key && aiUi.test, JSON.stringify(aiUi));
check("ai ui: reasoning levels", aiUi.reasoningOptions.join(",") === "auto,low,medium,high", aiUi.reasoningOptions.join(","));
const disabledAnswer = await main.eval(`(async () => {
  try { await window.__TAURI__.core.invoke('ai_analyze', { ids: [${JSON.stringify(shot.id)}], task: 'analyse_item', question: '' }); return 'unexpected success'; }
  catch (err) { return String(err); }
})()`);
check("ai: disabled provider refuses with a message", /ИИ выключен|Не задан API-ключ/.test(disabledAnswer), disabledAnswer);

// 7c. screenshot modal offers the AI actions
const modalUi = JSON.parse(await main.eval(`(async () => {
  SHOTS = await window.__TAURI__.core.invoke('shots_list');
  await openShot(${JSON.stringify(shot.id)});
  return JSON.stringify({
    buttons: [...document.querySelectorAll('[data-act="ai-run"]')].map(b => b.dataset.task),
    question: Boolean(document.querySelector('#ms-question')),
    answer: Boolean(document.querySelector('#ms-ai-answer')),
  });
})()`));
check("ai ui: modal actions", modalUi.buttons.includes("analyse_item") && modalUi.buttons.includes("next_upgrades") && modalUi.question && modalUi.answer, JSON.stringify(modalUi));
await main.eval(`(async () => { document.querySelector('#modal').hidden = true; activeShot = null; })()`);

// 8. AI request preparation (local only)
const ai = JSON.parse(await main.eval(`(async () => JSON.stringify(await window.__TAURI__.core.invoke('ai_prepare', { ids: [${JSON.stringify(shot.id)}], task: 'analyse_item' })))()`));
check("ai: request description written locally", existsSync(ai.path) && ai.json.includes("analyse_item"), ai.path);

// 9. overlay page
if (overlayTarget) {
  const ov = new CDP(overlayTarget.webSocketDebuggerUrl, "overlay");
  await ov.open();
  await ov.send("Runtime.enable");
  await sleep(800);
  const ovState = JSON.parse(await ov.eval(`JSON.stringify({ steps: CAMP.steps.length, mode: S.saved.overlay.mode, text: document.querySelector('#body').innerText.length, hidden: document.querySelector('#panel').hidden })`));
  check("overlay: campaign loaded", ovState.steps === 115, String(ovState.steps));
  check("overlay: panel rendered", ovState.hidden === false && ovState.text > 5, JSON.stringify(ovState));
  const modes = await ov.eval(`(async () => {
    const out = [];
    for (const m of ['compact','standard','extended']) {
      await window.__TAURI__.core.invoke('save_overlay', { overlay: { ...S.saved.overlay, mode: m } });
      await new Promise(r => setTimeout(r, 300));
      out.push(m + ':' + document.querySelector('#panel').className + ':' + document.querySelector('#body').innerText.length);
    }
    return out.join('|');
  })()`);
  check("overlay: all three modes render", modes.split("|").length === 3 && !modes.includes(":0"), modes);
  check("overlay: no page exceptions", ov.errors.length === 0, ov.errors.join("; "));
  ov.close();
}

// 9b. overlay visibility follows the setting
const vis = await main.eval(`(async () => {
  const s = JSON.parse(JSON.stringify(S.saved));
  const read = async () => {
    const d = await window.__TAURI__.core.invoke('diagnostics');
    const m = d.match(/overlay visible: (true|false)/);
    return m ? m[1] : 'none';
  };
  await window.__TAURI__.core.invoke('save_overlay', { overlay: { ...s.overlay, only_in_game: false, visible: true } });
  await new Promise(r => setTimeout(r, 700));
  const shown = await read();
  await window.__TAURI__.core.invoke('save_overlay', { overlay: { ...s.overlay, only_in_game: true, visible: false } });
  await new Promise(r => setTimeout(r, 700));
  const hidden = await read();
  await window.__TAURI__.core.invoke('save_overlay', { overlay: { ...s.overlay, only_in_game: true, visible: true } });
  return shown + '|' + hidden;
})()`);
check("overlay: shown when allowed", vis.startsWith("true"), vis);
check("overlay: hidden by the setting", vis.endsWith("false"), vis);

// 10. no uncaught exceptions in the main page
check("main: no uncaught exceptions", main.errors.length === 0, main.errors.slice(0, 3).join("; "));
check("main: no console errors", main.logs.length === 0, main.logs.slice(0, 3).join("; "));

// 11. diagnostics command works
const diag = await main.eval(`(async () => await window.__TAURI__.core.invoke('diagnostics'))()`);
check("diagnostics: report contains the log path", diag.includes("Client.txt") && diag.includes("campaign"), diag.split("\n").slice(0, 3).join(" / "));

// 12. state file on disk
const statePath = join(process.env.APPDATA, "io.github.bigbyon.enipoe2companion", "state.json");
check("state: state.json written", existsSync(statePath), statePath);
if (existsSync(statePath)) {
  const state = JSON.parse(readFileSync(statePath, "utf8"));
  check("state: profile persisted with progress", state.profiles[0].level === 8 && state.profiles[0].rewards.length >= 2, `lvl ${state.profiles[0].level}`);
  check("state: history persisted", state.history.length >= 4, String(state.history.length));
  check("state: screenshots persisted", state.shots.length >= 1, String(state.shots.length));
  check("state: no plain-text AI key", !JSON.stringify(state.ai).includes("sk-"), JSON.stringify(state.ai));
}

// 13. unknown (localized) zone offers a mapping and remembers it
w("[INFO Client 1] [SCENE] Set Source [Неизвестная долина]\n");
await waitFor(main, "S.zone_unknown === true && S.saved.profiles[0].zone === 'Неизвестная долина'");
const unknown = JSON.parse(await main.eval(`(async () => {
  go('campaign');
  await new Promise(r => setTimeout(r, 300));
  return JSON.stringify({ unknown: S.zone_unknown, zone: S.saved.profiles[0].zone, hasSelect: Boolean(document.querySelector('#map-zone')) });
})()`));
check("zone map: unknown zone detected", unknown.unknown === true && unknown.zone === "Неизвестная долина", JSON.stringify(unknown));
check("zone map: mapping control offered", unknown.hasSelect === true, JSON.stringify(unknown));
const mapped = JSON.parse(await main.eval(`(async () => {
  document.querySelector('#map-zone').value = 'The Red Vale';
  await window.__TAURI__.core.invoke('zone_map', { zone: 'The Red Vale' });
  await new Promise(r => setTimeout(r, 600));
  return JSON.stringify({ unknown: S.zone_unknown, aliases: S.saved.zone_aliases.map(a => a.alias), zone: S.saved.profiles[0].zone });
})()`));
check("zone map: alias remembered", mapped.aliases.includes("Неизвестная долина") && mapped.unknown === false, JSON.stringify(mapped));

main.close();

const failed = results.filter((r) => !r.ok);
log("\n" + "=".repeat(60));
log(`${results.length - failed.length}/${results.length} checks passed`);
if (failed.length) {
  log("FAILED:");
  for (const f of failed) log(" - " + f.name + (f.extra ? ": " + f.extra : ""));
  process.exit(1);
}
