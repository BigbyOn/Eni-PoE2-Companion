# PoE2 Overlay

Open-Source-Ingame-Overlay für Path of Exile 2 (Ersatz für poeoverlay.com ohne Werbung/Overwolf). Bisher nur Kampagnen-Guide. **Kein Electron** (Michaels Vorgabe), Tauri 2 + Rust.

## Struktur

| Pfad | Zweck |
|---|---|
| `src/main.rs` | Tauri-App: Zustand (`Saved` persistiert in `%APPDATA%\de.malura.poe2overlay\state.json`), Commands, Hotkeys, Tray |
| `src/log.rs` | `Client.txt`-Parser + Tail-Thread (250 ms), folgt dem Pfad aus `Shared.log` und wechselt die Datei live, Catch-up aus den letzten 64 KB |
| `src/guide.rs` | Zonen-Matching: Pointer springt beim Zonenwechsel vor (max. 8 Schritte, nie zurück, Stadt nur als direkt nächster Schritt) |
| `src/game.rs` | Win32: Fenster `Path of Exile 2` finden, Overlay-Fenster = genau das Panel, an Offset `x,y` (physische px) im Client-Bereich, nur bei Spiel im Vordergrund sichtbar, Ziehen über die echte Cursorposition |
| `ui/` | HTML/CSS/JS (ohne Build-Schritt), `ui/guide.json` = generierte Route |
| `src/clip.rs` | Text in die Windows-Zwischenablage (Regexe) |
| `ui/menu.*` | Seite für zwei Fenster: `menu` = Einstellungen (`Shift+Space`, fokussierbar, beim Schließen bekommt das Spiel den Fokus zurück), `regex` = Regex-Liste (nie fokussiert wie das Panel, sonst schließt sie beim Loslassen von Alt) |
| `scripts/make_icon.py` | erzeugt `icons/icon.ico` |

## Wichtig

- Versionen exakt gepinnt (`=2.12.1` tauri, `=2.7.1` tauri-build). Tauri-Unterkrates müssen zusammenpassen: 2.11.6 + neues tauri-runtime kompiliert nicht.
- Alle Fenster haben `"create": false` und entstehen in `setup` erst nach `manage()`: sonst rufen die Seiten Commands vor dem State auf ("state not managed") und das Panel bleibt unsichtbar. Debuggen der Seiten: `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333` setzen, dann per DevTools-Protokoll (`/json/list`, Node-WebSocket) Fehler lesen, ohne Eingaben.
- `focusable: false` im Fenster-Config: Klick ins Overlay lässt dem Spiel den Fokus. Nicht entfernen.
- Kein Klick-durch-Umschalten: `set_ignore_cursor_events(false)` entfernt `WS_EX_LAYERED` (weißer Blitz), und mit dauerhaft gesetztem `LAYERED` gehen Klicks durch das Panel ins Spiel (WebView2 zeichnet an der Layered-Fläche vorbei). Darum ist das Fenster nur so groß wie das Panel.
- Schritt erledigt per Belohnungszeile im Log (`: <char> has received +10% to [Resistances|Cold Resistance].`, `: You have received 2 Passive Skill Points.`): Tabelle `REWARDS` in `src/guide.rs` mappt Phrase im englischen Goal-Text nach `->` auf den Log-Text. Kills selbst stehen nicht im Log, Boss-Sprüche schon (`Beira of the Rotten Pack: Invader!`).
- Threads/Sperren: Der Global-Shortcut-Handler läuft auf dem Hauptthread unter der Plugin-Sperre (auch bei Released-Events), und `register`/`unregister_all` halten dieselbe Sperre, während sie auf den Hauptthread warten. Darum Hotkeys nur beim Start und in `save_settings` (Hauptthread) registrieren, nie in Aktionen und nie aus Nebenthreads (beides hat das Overlay schon eingefroren). Fenster-Getter (`scale_factor`, `is_visible`, `outer_size`) warten von Nebenthreads auf den Hauptthread: dabei nie `shared`/`snap` halten. Setter (`set_position`, `show`) blockieren nicht. Sync-Commands laufen auf dem Hauptthread.
- Panelgröße kommt per ResizeObserver vom JS in CSS-px, Rust rechnet mit `scale_factor` auf physische px um und setzt die Fenstergröße.
- Log-Pfad: sobald das Spielfenster da ist, `logs\Client.txt` neben der exe des Spielprozesses (Michael spielt über Steam auf `D:\Steam\...`, der Standalone-Log unter C: ist veraltet). Prüfen mit `cargo test -- --ignored` bei laufendem Spiel.
- Log-Zeilen-Formate stehen als Tests in `src/log.rs` (aus echtem PoE2-Log, 0.5.x).
- Inhalte der Route werden in `../poe2` gepflegt, nicht hier. `build.py` dort schreibt `dist/overlay/guide.json`, live unter `https://poe2speedrun.malura.de/overlay/guide.json`; das Overlay holt sie beim Start (Cache `guide.json` im Config-Ordner, Fallback `ui/guide.json`). Schritt-Index hängt am Index: neue Schritte verschieben gespeicherten Fortschritt.
- Route-Modus (`all`/`buffs`/`must`) wie die Presets der Webseite; Regel steht doppelt in `src/guide.rs::view` und `ui/app.js::shown`.

## Testen

`cargo test`. E2E ohne Spiel: PowerShell-WinForms-Fenster öffnen und das Overlay mit `POE2OVERLAY_GAME_TITLE=<Titel>` darauf zeigen lassen (kollidiert so nicht mit einem laufenden Spiel). **Keine simulierten Klicks/Tasten, solange PoE2 läuft:** Michael kann jederzeit zurück ins Spiel wechseln, Eingaben landen dann im Bosskampf. Nur lesend prüfen (GetWindowRect, Styles) oder Michael testen lassen, in `state.json` `client_log` auf eine Test-Datei setzen, SCENE-Zeilen anhängen, Hotkeys per `keybd_event`. In PowerShell-P/Invoke `[NullString]::Value` statt `$null` an `FindWindow` geben.

## Repos und Release

- Remotes: `origin` = `git@vault:maluramichael/poe2overlay.git`, `github` = https://github.com/maluramichael/poe2overlay (öffentlich). Immer beide pushen.
- `.github/workflows/build.yml`: auf jedem Push `cargo test` + Installer (NSIS `setup.exe` + `.msi`) als Artefakt; Tag `vX.Y.Z` erstellt ein GitHub-Release, Tag muss zu `version` in `tauri.conf.json` passen (Workflow prüft das). Versionen in `tauri.conf.json` und `Cargo.toml` gemeinsam anheben.
- Lokal Installer bauen ohne das laufende Overlay zu stoppen: `CARGO_TARGET_DIR=target-bundle npx --yes @tauri-apps/cli@2.12.1 build`.

Commits mit `--author="Michael Malura <michael@malura.de>"`, ohne Co-Authored-By.
