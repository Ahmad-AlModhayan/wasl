# Setup: Google Cloud OAuth client (one-time, ~10 minutes)

The app talks to Google Drive with your own Google Cloud project. Nothing is sent anywhere else.
Because the app is personal and never published, it does not need Google's app verification.

## 1. Create the project and enable the Drive API
1. Open https://console.cloud.google.com/ and create a project, e.g. **Drive Sync personal**.
2. **APIs & Services → Library** → search **Google Drive API** → **Enable**.

## 2. Configure the OAuth consent screen (Google Auth Platform)
1. **APIs & Services → OAuth consent screen** (new UI: **Google Auth Platform → Get started**).
2. App name: `Wasl`, user support email: a Google Group you manage (or your own Google account address), audience: **External**, contact email: your Gmail address. Finish.
3. **Data access → Add or remove scopes** → filter for `drive` → tick `https://www.googleapis.com/auth/drive` ("See, edit, create, and delete all of your Google Drive files") → Update → Save.
4. **Branding** (Google Auth Platform → Branding): production apps need a home page and privacy policy on an
   authorized domain. Wasl's pages are the Kahf site in `site/` (Astro, English and Arabic), served at
   **https://wasl.kahflabs.com**:

   | Field | Value |
   |---|---|
   | App name | **Wasl** (must match the home page and the app) |
   | Application home page | `https://wasl.kahflabs.com` |
   | Privacy policy | `https://wasl.kahflabs.com/privacy` |
   | Terms of service | `https://wasl.kahflabs.com/terms` |
   | Authorized domain | `kahflabs.com` |

5. **Audience → Publishing status → Publish app** (choose "In production"). Do **not** submit for verification. Production status avoids the 7-day token expiry that "Testing" apps have. The first time you sign in, Google shows "Google hasn't verified this app": tap **Advanced → Go to Wasl (unsafe)**. That warning is expected for an unverified personal app; the "unsafe" wording refers to the missing review, not to the app.
   - If you prefer to stay in **Testing**, add your Gmail address under **Audience → Test users**. You will then have to re-authorize every 7 days.

## 3. Create the Android OAuth client
1. **Clients → Create client** (older UI: **Credentials → Create credentials → OAuth client ID**).
2. Application type: **Android**.
3. Name: `Drive Sync Android`.
4. Package name: `com.almodhayan.drivesync`
5. SHA-1 certificate fingerprint: the fingerprint of the key that signs the APK (see [Signing keys](#signing-keys)).
   Create **one Android client per fingerprint**, all with the same package name:

   Register one Android client per signing key you build with (your release key, and each machine's debug key), using
   the package name and that key's SHA-1 (commands below).

   To print a fingerprint again (or for a new machine's debug key):
   ```bash
   keytool -list -v -keystore ~/.android/debug.keystore -alias androiddebugkey -storepass android | grep SHA1
   keytool -list -v -keystore ~/.android/drivesync-release.jks -alias drivesync | grep SHA1   # asks for the release password
   ```
6. Create. No client secret or JSON file is needed in the app: Google Play services matches the package name and signature automatically.
7. Wait 2–5 minutes for the client to propagate.

## 4. Build and install

Build the sync engine, background daemon, and CLI from this repository (`engine/README.md`):

```sh
cd engine
cargo build --workspace --release
```

Official native UI clients (Android and desktop) build from separate client repositories against `wasl-ffi` / `wasl-ipc`. Only release Android builds go on a phone. Debug and release share the package name but not the key, so switching the phone between them means uninstalling (and losing the app's data).

For a standalone Android build in a client checkout:

Mac:
```bash
export JAVA_HOME=$(/usr/libexec/java_home -v 21)
./gradlew :app:assembleRelease
adb install -r app/build/outputs/apk/release/app-release.apk
```
Windows (Git Bash, JDK 21):
```bash
export JAVA_HOME="$HOME/.jdk/jdk-21.0.8"
./gradlew :app:assembleRelease
adb install -r app/build/outputs/apk/release/app-release.apk
```
A release built on either machine installs over one built on the other, because both use the shared release key.

## Signing keys
| Key | Where | Used for |
|---|---|---|
| Release | `~/.android/drivesync-release.jks` (alias `drivesync`, PKCS12, RSA 4096, valid to 2056) | `assembleRelease` on every machine |
| Debug | `~/.android/debug.keystore`, Android's default, different per machine | `assembleDebug`, emulator and local testing |

The release password is in the user-level `~/.gradle/gradle.properties` (never the repo's):
```properties
driveSync.releaseKeyPassword=<password>
# optional, if the keystore is somewhere else:
# driveSync.releaseStoreFile=/path/to/drivesync-release.jks
```
`assembleRelease` stops with "Release signing key missing" if either is absent, rather than producing an unsigned APK.

**Back up the keystore and its password together** (e.g. in a password manager). Without them no
future build can update the installed app; you would have to uninstall it and set everything up again.

Setting up another machine: copy `drivesync-release.jks` to its `~/.android/`, add the
`driveSync.releaseKeyPassword` line to its `~/.gradle/gradle.properties`, then register that machine's
debug SHA-1 as another Android client if you want debug builds to sign in.

## 5. First run
1. Grant **All files access**, notifications, and battery optimization exemption in the onboarding.
2. **Add Google account** → pick your account → allow Drive access (accept the unverified-app screen once).
3. The app indexes your Drive once (a minute for large accounts), then create folder pairs.

## Troubleshooting
| Symptom | Cause / fix |
|---|---|
| "DEVELOPER_ERROR (10)" when adding an account | No Android OAuth client matches the installed APK's package name and SHA-1. Check `apksigner verify --print-certs app-release.apk` against the table in step 3. |
| `INSTALL_FAILED_UPDATE_INCOMPATIBLE` from `adb install` | The phone has a build signed with a different key (e.g. a debug build, or an old build signed with a machine's debug key). Uninstall it first; this wipes the app's data. |
| "Publish app" is disabled: "complete the configuration on the Branding page" | Production apps need an app home page and privacy policy link on an authorized domain. See step 2.4: the pages in `site/` on wasl.kahflabs.com (public, not behind the Access login), `kahflabs.com` verified in Search Console, then publish. Or stay in Testing with yourself as a test user. |
| "Access blocked: Wasl has not completed the Google verification process" | App is in Testing and your email is not a test user, or the scope was not added. Publish to production (step 2.5) or add yourself as a test user. |
| Sign-in works but sync fails with "insufficient permissions" | The `auth/drive` scope was not added on the consent screen. Add it, then in the app: Accounts → Re-authorize. |
| Sync stops after a week | The consent screen is still in Testing. Publish to production, then Accounts → Re-authorize once in the app. |
| Nothing syncs in the background | Diagnostics screen: battery optimization must be off for the app; on Xiaomi/Huawei/Oppo/Vivo also enable Autostart. |
