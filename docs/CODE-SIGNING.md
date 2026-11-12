# Подпись кода (SmartScreen) — зачем и как

## Зачем

Windows проверяет подпись установщика и exe. Без подписи SmartScreen показывает «Windows защитила ваш компьютер» и прячет запуск за кнопкой «Подробнее → Выполнить в любом случае». Пользователи пугаются, корпоративные политики иногда блокируют неподписанные файлы, а антивирусы относятся к ним подозрительнее.

Подпись даёт:

- нормальный запуск без предупреждений (после набора репутации);
- проверяемое авторство: хеш файла привязан к сертификату;
- возможность автоматической проверки обновлений в будущем (WinVerifyTrust).

Подпись **не** делает программу безопаснее сама по себе — она подтверждает, кто собрал файл, и что файл не изменён после подписи.

## Что нужно

1. **Сертификат подписи кода** (Code Signing Certificate, CSC).
   - **OV** (Organization Validation) — от ~$200–400/год, обычный, репутация набирается.
   - **EV** (Extended Validation) — дороже, репутация SmartScreen сразу.
   - **Azure Trusted Signing** (Microsoft) — подписка, подпись через облако, без USB-токена; для ИП/юрлиц.
   - Для личного проекта без юрлица чаще берут **open-source-варианты**: SignPath Foundation (бесплатно для OSS), Certum Open Source Code Signing (недорого).
2. **Хранилище ключа**: USB-токен (обязателен для OV/EV по новым правилам) или облачный HSM (Azure Trusted Signing). Ключ нельзя держать в репозитории.
3. **Timestamp-сервер** (например, `http://timestamp.digicert.com`) — чтобы подпись оставалась действительной после истечения сертификата.

## Как подписать локально

Самый простой путь — `signtool.exe` из Windows SDK:

```powershell
signtool sign /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 `
  /sha1 <отпечаток_сертификата> `
  dist\Eni-PoE2-Companion-Setup.exe dist\Eni-PoE2-Companion.msi
```

Проверка:

```powershell
Get-AuthenticodeSignature dist\Eni-PoE2-Companion-Setup.exe | Format-List Status, SignerCertificate
```

Ожидается `Status: Valid`.

## Как подключить к CI

В Tauri есть встроенная поддержка подписи Windows-бандлов. В `tauri.conf.json`:

```jsonc
"bundle": {
  "windows": {
    "certificateThumbprint": "<отпечаток>",
    "digestAlgorithm": "sha256",
    "timestampUrl": "http://timestamp.digicert.com"
  }
}
```

Для облачной подписи (Azure Trusted Signing) добавляется шаг подписи до публикации: официальный экшен `azure/trusted-signing-action` подписывает `dist/*.exe` и `dist/*.msi`, а секреты (`AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, endpoint) хранятся в GitHub Secrets.

Рекомендуемая схема в нашем workflow (`.github/workflows/build.yml`):

1. собрать бандлы (`npx tauri build`);
2. скопировать в `dist/` под финальными именами;
3. если заданы секреты подписи — подписать `dist/*`;
4. посчитать SHA‑256 и опубликовать.

Пока секретов нет, шаг подписи просто пропускается — релиз остаётся неподписанным (как в 0.1.0/0.2.0), и об этом честно написано в release notes и README.

## Частые вопросы

**Можно ли самоподписанным сертификатом?** Технически да (`New-SelfSignedCertificate` + `signtool`), но SmartScreen и другие машины такой сертификат не доверяют: предупреждение останется. Годится только для внутренних тестов.

**Нужно ли подписывать каждую сборку?** Да, подпись делается после сборки; при каждом релизе — заново, с timestamp.

**Что подписывать?** Установщики (`Setup.exe`, `.msi`) и желательно сам `eni-poe2-companion.exe` внутри бандла — Tauri подписывает и то, и другое при наличии настроек.

**Сколько ждать снятия предупреждения SmartScreen?** Для OV — пока наберётся репутация (дни-недели скачиваний). EV и Azure Trusted Signing дают мгновенную репутацию.
