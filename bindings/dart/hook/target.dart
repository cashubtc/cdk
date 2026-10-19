import 'package:code_assets/code_assets.dart';

/// The `prebuilt/` subdirectory holding the library for [config]'s target.
///
/// Mirrors the target list the release workflow builds. iOS splits by SDK
/// because the device and simulator slices are different Rust targets.
String targetTriple(CodeConfig config) {
  return switch ((config.targetOS, config.targetArchitecture)) {
    (OS.android, Architecture.arm64) => 'aarch64-linux-android',
    (OS.android, Architecture.arm) => 'armv7-linux-androideabi',
    (OS.android, Architecture.x64) => 'x86_64-linux-android',
    (OS.iOS, Architecture.arm64)
        when config.iOS.targetSdk == IOSSdk.iPhoneSimulator =>
      'aarch64-apple-ios-sim',
    (OS.iOS, Architecture.arm64) => 'aarch64-apple-ios',
    (OS.iOS, Architecture.x64) => 'x86_64-apple-ios',
    (OS.windows, Architecture.x64) => 'x86_64-pc-windows-msvc',
    (OS.linux, Architecture.arm64) => 'aarch64-unknown-linux-gnu',
    (OS.linux, Architecture.x64) => 'x86_64-unknown-linux-gnu',
    (OS.macOS, Architecture.arm64) => 'aarch64-apple-darwin',
    (OS.macOS, Architecture.x64) => 'x86_64-apple-darwin',
    _ => throw UnsupportedError(
        'Unsupported target: ${config.targetOS} / ${config.targetArchitecture}'),
  };
}

/// The link mode to satisfy [config]'s preference with.
LinkMode linkModeFor(CodeConfig config) {
  return switch (config.linkModePreference) {
    LinkModePreference.dynamic ||
    LinkModePreference.preferDynamic =>
      DynamicLoadingBundled(),
    LinkModePreference.static ||
    LinkModePreference.preferStatic =>
      StaticLinking(),
    _ => throw UnsupportedError(
        'Unsupported LinkModePreference: ${config.linkModePreference}'),
  };
}
