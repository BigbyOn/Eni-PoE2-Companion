# CLAUDE.md — Eni-PoE2-Companion

Помощник для Path of Exile 2: Rust + Tauri 2 + системный WebView2, без телеметрии; сеть — только для ИИ по кнопке.
Производная работа от [poe2overlay](https://github.com/maluramichael/poe2overlay) (MIT, © 2026 Michael Malura) — уведомления сохранены в [LICENSE](LICENSE) и [NOTICE](NOTICE).

Документация: [README.md](README.md), [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), [docs/BUILD.md](docs/BUILD.md), [docs/SECURITY-AUDIT.md](docs/SECURITY-AUDIT.md), [docs/KNOWN-LIMITATIONS.md](docs/KNOWN-LIMITATIONS.md), [docs/AI-PROVIDER.md](docs/AI-PROVIDER.md).

## Структура

| Путь | Назначение |
|---|---|
| `src/main.rs` | состояние (`Snapshot`), команды Tauri, горячие клавиши, трей, создание окон |
| `src/model.rs` | `state.json`: настройки, профили, история, скриншоты, DPAPI-блоб ключа, санитизация |
| `src/logparse.rs` | разбор строк журнала (EN/RU, UTF‑8/UTF‑16LE), кодировки |
| `src/tail.rs` | слежение за файлом, «догоняющее» чтение 64 КБ, усечение/смена файла |
| `src/discover.rs` | поиск журнала: процесс игры, Steam (реестр + `libraryfolders.vdf`), standalone |
| `src/campaign.rs` | маршрут кампании, шаг, режимы «все/только обязательные», алиасы и id областей |
| `src/builds.rs` | JSON билдов, валидация, пользовательские билды |
| `src/screens.rs` | PNG, миниатюры 320 px, SHA‑256, папка скриншотов |
| `src/capture.rs` | GDI `BitBlt` клиентской области игры |
| `src/game.rs` | окно игры, размещение оверлея, перетаскивание, показ/скрытие |
| `src/ai.rs` | ИИ: DeepSeek/OpenAI по кнопке, сборка запросов, DPAPI для ключа |
| `src/item.rs` | разбор текста предмета из буфера (EN/RU) и локальная оценка под билд |
| `src/trade.rs` | проверка цены через официальный trade API (по кнопке, с лимитом частоты) |
| `src/pob.rs` | импорт PoB-кода (base64+zlib+XML) в локальный билд, с лимитами и валидацией |
| `src/clip.rs` | запись в буфер обмена |
| `ui/index.html`, `ui/app.js`, `ui/app.css`, `ui/i18n.js` | главное окно: 8 разделов, RU/EN |
| `ui/overlay.*` | оверлей трёх режимов |
| `assets/campaign/campaign.json` | маршрут (генерируется `tools/gen_campaign.py`) |
| `assets/builds/*.json` | билды |
| `tools/` | генератор маршрута, аудит зависимостей, фальшивое окно и сквозной тест |

## Инварианты (ломать нельзя)

1. **Порядок блокировок**: `campaign` → `snap` → `builds` → `shared`. Никогда не брать `snap` дважды (см. `App::store`). Диагностика берёт блокировки по одной.
2. **`show()`/`hide()` — вне блокировки `shared`**: эти вызовы ждут главный поток и иначе вешают приложение.
3. **Горячие клавиши** регистрируются только на главном потоке: в `setup` и в `save_settings`. Не в обработчике хоткея и не из фонового потока.
4. **Окна** создаются в `setup` после `manage()` (`"create": false` в `tauri.conf.json`), иначе страницы вызовут команды до инициализации состояния.
5. **Оверлей** — `focusable: false`, `alwaysOnTop`, размер окна равен панели; фокус игры не перехватывается.
6. **Сеть**: только по явной кнопке — ИИ (DeepSeek/OpenAI) и проверка цены (официальный trade API). Фиксированные endpoint'ы, HTTPS, без редиректов, ключ в DPAPI, лимит частоты. Никаких фоновых запросов, телеметрии и автообновлений; маршрут и билды обновляются только с релизом.
7. **Ссылки** открываются только через `link_open` (https + белый список хостов).
8. **Локализация**: русский — основной язык; в UI нет смеси языков, кроме имён собственных (зоны, боссы, предметы) — они остаются в написании клиента.
9. **Лицензия**: сохранять MIT-уведомления и атрибуцию poe2overlay в README/NOTICE/CHANGELOG.

## Проверка

```powershell
cargo test                    # 82 юнит-теста (журналы, локации, предметы, trade API со статами и поиском, чек-лист локации, PoB, ИИ-запросы, DPAPI)
cargo build                   # debug
npx --yes @tauri-apps/cli@2.12.1 build   # NSIS + MSI
```

Сквозной тест интерфейса (не запускать, если пользователь играет — окна появляются на экране):

```powershell
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9333'
$env:ENI_COMPANION_GAME_TITLE='EniTestWindow'   # изолирует от настоящей игры
node tools/smoke.mjs <временная папка>
```

`tools/fake_game.ps1` создаёт тестовое окно (всегда сверху — закрывать после теста).

## Репозиторий и релиз

- `origin` = https://github.com/BigbyOn/Eni-PoE2-Companion (наш репозиторий).
- `upstream` = https://github.com/maluramichael/poe2overlay (только чтение, для сравнения; ничего не пушить).
- Ветка разработки: `feature/eni-companion-v01`; PR — в `main`.
- Версия меняется одновременно в `Cargo.toml` и `tauri.conf.json`; CI сверяет тег `vX.Y.Z` с версией.
- Тег `v*` собирает установщики и публикует GitHub Release с `Eni-PoE2-Companion-Setup.exe`, `Eni-PoE2-Companion.msi`, `SHA256SUMS.txt` и текстом `docs/RELEASE_NOTES.md`.
