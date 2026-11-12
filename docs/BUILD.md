# Сборка Eni-PoE2-Companion

## Что нужно

| Компонент | Версия / примечание |
|---|---|
| Windows | 10 или 11 x64 |
| Rust | stable (`rust-toolchain.toml` фиксирует канал; MSVC toolchain) |
| Visual Studio Build Tools | «Desktop development with C++» (MSVC + Windows SDK) |
| Node.js | 18+ (только для CLI Tauri и `npx`) |
| WebView2 | есть в Windows 10/11; для Windows 10 при необходимости — Evergreen Runtime |

Проверка окружения:

```powershell
rustc --version      # stable
node --version
npx --yes @tauri-apps/cli@2.12.1 --version
```

## Отладочная сборка

```powershell
cd Eni-PoE2-Companion
cargo test           # юнит-тесты
cargo run            # запуск приложения с включённой консолью
```

Пересборка встроенных страниц (`ui/`, `assets/`) подхватывается автоматически:
`build.rs` объявляет их через `cargo:rerun-if-changed`. Если менялся только UI, а
бинарник не пересобрался, используйте `cargo clean -p eni-poe2-companion`.

## Установщики

```powershell
npx --yes @tauri-apps/cli@2.12.1 build
```

Результат:

| Файл | Что это |
|---|---|
| `target/release/bundle/nsis/Eni-PoE2-Companion_0.1.0_x64-setup.exe` | NSIS-установщик (RU/EN, выбор языка) |
| `target/release/bundle/msi/Eni-PoE2-Companion_0.1.0_x64_en-US.msi` | MSI (WiX) |

Для именования, как в релизе:

```powershell
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item target\release\bundle\nsis\*.exe dist\Eni-PoE2-Companion-Setup.exe
Copy-Item target\release\bundle\msi\*.msi dist\Eni-PoE2-Companion.msi
Get-FileHash dist\* -Algorithm SHA256 | ForEach-Object { "$($_.Hash.ToLower())  $(Split-Path $_.Path -Leaf)" } | Set-Content dist\SHA256SUMS.txt
```

Первая сборка MSI/NSIS скачивает инструменты WiX и NSIS — нужен интернет.
Приложение не подписывается; для подписи добавьте сертификат в конфигурацию Tauri
(`bundle.windows.certificateThumbprint`) или подписывайте артефакты шагом CI.

## CI

`.github/workflows/build.yml` на каждый push и PR:

1. `cargo test --locked`;
2. проверка соответствия тега и версии в `tauri.conf.json`;
3. `npx @tauri-apps/cli build`;
4. переименование артефактов в `Eni-PoE2-Companion-Setup.exe` / `Eni-PoE2-Companion.msi` + `SHA256SUMS.txt`;
5. загрузка артефактов;
6. на теге `v*` — GitHub Release с установщиками и контрольными суммами.

## Обновление встроенных данных

| Данные | Как обновить |
|---|---|
| Маршрут кампании | правится в `tools/campaign_ru.py`, затем `python tools/gen_campaign.py` → `assets/campaign/campaign.json` |
| Билды | `assets/builds/*.json` (или пользовательские в папке данных) |
| Иконка | `python scripts/make_icon.py` |

Версию приложения меняйте сразу в `Cargo.toml` и `tauri.conf.json` — CI сверяет тег.

## Полезные отладочные приёмы

- Точка прицела оверлея на другое окно (не трогая игру): переменная
  `ENI_COMPANION_GAME_TITLE=Заголовок окна`.
- DevTools страниц: запустить с `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9333`
  и открыть `http://[::1]:9333/json/list`.
- Сквозной тест: `node tools/smoke.mjs <временная папка>` (см. комментарий в файле).
- Фальшивое игровое окно для тестов: `tools/fake_game.ps1` (переменная `ENI_FAKE_TITLE`).
