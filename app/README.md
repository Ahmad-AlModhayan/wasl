# Wasl for Android

The Android app (Gradle module `app`, Kotlin, Jetpack Compose, application id `com.kahflabs.wasl`, minSdk 30). It
provides the UI and Android integration; sync decisions are made by the Rust engine in [`engine/`](../engine/README.md),
linked in through UniFFI bindings.

## Build

Needs JDK 17 or newer (CI uses 17), the Android SDK, a Rust toolchain, and the Android NDK `28.2.13676358`. Gradle
builds the engine for `arm64-v8a` and `x86_64` with `cargo-ndk`:

```sh
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk --version 4.1.2 --locked
sdkmanager --install "ndk;28.2.13676358"
./gradlew :app:assembleDebug
```

To sign in to Google you need your own OAuth client; follow [docs/SETUP.md](../docs/SETUP.md). Release builds
(`./gradlew :app:assembleRelease`) need your own signing key, see
[SETUP.md, Signing keys](../docs/SETUP.md#signing-keys). Install with
`adb install -r app/build/outputs/apk/release/app-release.apk`.

## Tests

```sh
./gradlew :app:testDebugUnitTest
```

JVM tests only (Robolectric where needed); no device is required.

## Docs

[USER_GUIDE](../docs/USER_GUIDE.md) · [ARCHITECTURE](../docs/ARCHITECTURE.md) · [ENCRYPTION](../docs/ENCRYPTION.md) ·
[AUTOMATION](../docs/AUTOMATION.md) · [SETUP](../docs/SETUP.md)

## Licence

Wasl is source-available under the [PolyForm Shield License 1.0.0](../LICENSE). It is not open source by the OSI definition.

## Contributing, security, trademarks

- Contributing: [CONTRIBUTING.md](../CONTRIBUTING.md). Contributors sign a one-time [CLA](../CLA.md).
- Security: report problems privately, see [SECURITY.md](../SECURITY.md).
- Trademarks: [TRADEMARKS.md](../TRADEMARKS.md).
