# macOS Signing and Notarisation

> macOS says "Apple cannot check it for malicious software" when you open Cinder / Warden —
> that is not a broken app. It means **the build carries no Apple Developer ID signature and no
> notarisation ticket**. This page is about turning that off for good.

---

## 1. Three different things, three different fixes

| What you see | What it is | Who fixes it |
| :--- | :--- | :--- |
| "…Not Opened. Apple cannot check it for malicious software." | **Gatekeeper** refusing an unsigned app | signing + notarisation |
| "…is damaged and can't be opened." | The same thing, different wording | signing + notarisation |
| A keychain password prompt | A keychain item's ACL | Not this page — see [troubleshooting](../troubleshooting.md) |

Signing proves **who built it**. Notarisation is Apple's malware scan proving **it is not malware**.
You need both: signing alone changes the dialog to "is damaged", and notarisation without a
signature is refused outright.

**Notarisation requires a signature**, and that signature must carry the hardened runtime and a
secure timestamp. Miss any one of the three and Apple rejects the upload.

---

## 2. Get the certificate

### 2.1 You need the Developer Program

**A free Apple ID is not enough.** You need an [Apple Developer Program](https://developer.apple.com/programs/)
membership — USD 99/year (individual or organisation both work).

Enrolling from a mainland-China account often fails at the payment step. The way that works more
often is to **register with a phone number** (pick "no US App Store account" → pick the China
mainland number), which routes to a payment method that actually goes through.

Once enrolled, [developer.apple.com/account](https://developer.apple.com/account/) is where
everything lives.

### 2.2 Find the Team ID

Avatar in the top right → "Membership details" → **Team ID**: ten uppercase letters and digits,
like `ABCDE12345`. You will need this value several times below.

### 2.3 Create the Developer ID Application certificate

**Option A: Xcode (easiest)**

1. Xcode → Settings → Accounts → pick your Apple ID → Manage Certificates… → **+** at the
   bottom left → **Developer ID Application** → Done
2. The certificate and its private key land in your keychain.

Check it:

```bash
security find-identity -v -p codesigning | grep "Developer ID Application"
```

You are done when this prints:

```
1) A1B2C3D4E5F6... "Developer ID Application: Your Name (ABCDE12345)"
```

**Option B: the developer portal (no Xcode)**

1. [Certificates](https://developer.apple.com/account/resources/certificates) → **+** at the top
   left → Software → **Developer ID Application** → Continue
2. It asks for a **CSR file**. On macOS: Keychain Access → Certificate Assistant → Create a
   Certificate Request, save it as `.certSigningRequest`
3. Upload the CSR → Continue → Download → double-click the `.cer` to install it

> ⚠️ Creating a Developer ID certificate needs the **Account Holder** role. In a team, an Admin
> member will not see this option.

---

## 3. Export it for CI

CI cannot read your keychain, so the certificate has to be exported.

### 3.1 Export the .p12

Keychain Access → find the **Developer ID Application** certificate → double-click → expand the
arrow next to "Export" → export as **PKCS#12 (.p12)** → **set a password** (you will need it
later, so do not lose it).

Turn it into base64 — GitHub Secrets store base64 so a stray newline cannot break the YAML:

```bash
openssl base64 -in ~/Desktop/DeveloperIDApplication.p12 -out ~/Desktop/cert-base64.txt
```

### 3.2 Get the signing identity

This is the **string inside the quotes** from the `security find-identity` output in 2.3:

```
"Developer ID Application: Your Name (ABCDE12345)"
```

That is the value of `APPLE_SIGNING_IDENTITY`. **The contents of the quotes, without the quotes
themselves.**

---

## 4. Notarisation credentials (pick one)

### Option A: App Store Connect API key (recommended)

The most reliable in CI: no password in the log, no two-factor prompt in the middle of a release.

1. [App Store Connect → Users and Access → Integrations](https://appstoreconnect.apple.com/access/integrations/api)
   → **+** → **App Store Connect** → name it → role **Developer** → Continue → **Download**
   (the `.p8` file — **this download works exactly once**)
2. The **Issuer ID** (a UUID) is shown above the keys table.

That gives you three values:

- `APPLE_API_KEY` = the Key ID (ten alphanumeric characters)
- `APPLE_API_ISSUER` = the Issuer ID
- `APPLE_API_KEY_P8` = the **base64** of the `.p8` file

```bash
openssl base64 -in ~/Downloads/AuthKey_XXXXXXXXXX.p8 -out ~/Desktop/p8-base64.txt
```

### Option B: Apple ID + app-specific password

1. [appleid.apple.com](https://appleid.apple.com) → sign in → "Sign-In and Security" →
   **App-Specific Passwords** → create one → you get a 16-character password
   (`abcd-efgh-ijkl-mnop`, **shown only once**)
2. `APPLE_ID` = your Apple ID email
3. `APPLE_PASSWORD` = that app-specific password
4. `APPLE_TEAM_ID` = the Team ID from 2.2

> An app-specific password works **only** for notarisation. It cannot sign you into the
> developer portal.

---

## 5. Put it in GitHub Secrets

**Set it up in both repositories** — Cinder and Warden each have their own release pipeline:

```bash
# in the Warden repository
cd /path/to/warden
gh secret set APPLE_CERTIFICATE        < ~/Desktop/cert-base64.txt
gh secret set APPLE_CERTIFICATE_PASSWORD --body '<the password you set when exporting the .p12>'
gh secret set APPLE_SIGNING_IDENTITY    --body 'Developer ID Application: Your Name (ABCDE12345)'
gh secret set APPLE_TEAM_ID             --body 'ABCDE12345'

# Option A
gh secret set APPLE_API_KEY             --body 'XXXXXXXXXX'
gh secret set APPLE_API_ISSUER          --body 'xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx'
gh secret set APPLE_API_KEY_P8          < ~/Desktop/p8-base64.txt

# or Option B — do not mix with Option A
gh secret set APPLE_ID                  --body 'you@example.com'
gh secret set APPLE_PASSWORD            --body 'abcd-efgh-ijkl-mnop'
```

Use `<` to read from a file, `--body` for a value. **Do not type secrets on the command line** —
that writes them into your shell history. Safer:

```bash
gh secret set APPLE_PASSWORD < <(security find-generic-password -s ... -w)
```

or just run `gh secret set APPLE_PASSWORD` with no arguments and let it prompt (input is not
echoed).

### Confirm

```bash
gh secret list | grep APPLE
```

---

## 6. What CI does

Warden is a Tauri app, so **Tauri's own bundler** does the signing and the notarisation as part of
`tauri build` — there is no script to write. The pipeline's job is to hand it the credentials:

```
publish job
  ├─ Configure Apple signing      ← puts the secrets into $GITHUB_ENV, writes the .p8 out
  ├─ Verify notarisation creds    ← a half-configured set fails here, not 20 minutes later
  └─ Build and publish (tauri-action)
       └─ tauri build
            ├─ import the certificate into a temporary keychain (and set the partition list)
            ├─ sign the .app
            ├─ notarytool submit --wait
            └─ stapler staple
```

The variables Tauri reads are exactly those above: `APPLE_CERTIFICATE`,
`APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, and for notarisation either
`APPLE_API_KEY` / `APPLE_API_ISSUER` / `APPLE_API_KEY_PATH` or `APPLE_ID` / `APPLE_PASSWORD` /
`APPLE_TEAM_ID`.

**Every step fails the job.** That is deliberate: an un-notarised dmg still installs, it just makes
every user dismiss a dialog once. That failure is invisible in the release pipeline — only users
find it for you.

---

## 7. If no certificate is configured

**A missing certificate does not fail the release**, but it prints a warning and the artifact is
unsigned.

To get past an unsigned dmg:

```bash
xattr -dr com.apple.quarantine "/Applications/Cinder.app"
```

Or right-click → **Open** → **Open** again in the dialog that follows. Or install with the
one-line script: `curl` writes no quarantine flag, so the app opens directly.

> **Do not run `sudo spctl --master-disable`.** That switches off download checking for the whole
> machine — every app's protection traded for one app's convenience.

---

## 8. Troubleshooting

**"the imported certificate does not provide identity"**
`APPLE_SIGNING_IDENTITY` does not match the certificate. Check the exact string with
`security find-identity -v -p codesigning` — the contents of the quotes, without the quotes.

**"security import" fails with a wrong-password error**
`APPLE_CERTIFICATE_PASSWORD` is the password you set **when exporting the .p12**, not your Apple
ID password.

**codesign hangs forever**
The `set-key-partition-list` step did not run. It is what stops codesign asking "allow this
application to use your private key".

**Notarisation says "The executable does not have a secure timestamp"**
The signature is missing `--timestamp`.

**Notarisation says "The binary is not signed with the hardened runtime"**
The signature is missing `--options runtime`.

**Notarisation says "The signature does not include a secure timestamp"**
Same, or the certificate used was not a Developer ID Application certificate.

**Notarisation says "package does not contain a valid signature"**
The .dmg was signed instead of the executable inside the app. Sign from the inside out: the
executables under `Contents/MacOS/` first, then the `.app`.

**"The issuer ID is invalid" with an App Store Connect key**
`APPLE_API_ISSUER` wants the Issuer ID (the UUID), not the Key ID. They are not interchangeable.

**Renewing the certificate**
Apps signed with an old certificate keep working for anyone who installed them before it expired.
To sign a new build, delete the old `v*` tag and create a new one.

---

## Related

- [Troubleshooting](../troubleshooting.md) — the keychain prompt, where the key lives, 401/404
- [Getting Started](getting-started.md) — installation
