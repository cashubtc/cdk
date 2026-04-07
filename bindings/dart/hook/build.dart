import 'dart:io';

import 'package:code_assets/code_assets.dart';
import 'package:hooks/hooks.dart';
import 'package:native_toolchain_rust/native_toolchain_rust.dart';
import 'package:path/path.dart' as p;

void main(List<String> args) async {
  await build(args, (input, output) async {
    if (!input.config.buildCodeAssets) return;

    final codeConfig = input.config.code;
    final targetTriple = _targetTriple(codeConfig);
    final linkMode = _linkMode(codeConfig);
    final packageRoot = p.fromUri(input.packageRoot);
    final libFileName =
        codeConfig.targetOS.libraryFileName('cdk_ffi_dart', linkMode);
    final prebuiltPath =
        p.join(packageRoot, 'prebuilt', targetTriple, libFileName);

    if (File(prebuiltPath).existsSync()) {
      // Pre-built binary found — skip cargo build.
      final outputPath =
          p.join(p.fromUri(input.outputDirectory), libFileName);
      await File(prebuiltPath).copy(outputPath);

      output.assets.code.add(
        CodeAsset(
          package: input.packageName,
          name: 'uniffi:cdk',
          linkMode: linkMode,
          file: Uri.file(outputPath),
        ),
      );
      return;
    }

    // No pre-built binary — fall back to building from source via cargo.
    // native_toolchain_rust replaces the process environment entirely when
    // spawning cargo (Dart's Process.run with an explicit environment map).
    // On Linux this results in an empty environment, so cargo can't find
    // system libraries like OpenSSL. Forward the full parent environment so
    // nix-provided paths (pkg-config, openssl, etc.) reach cargo's build
    // scripts.
    final env = Map<String, String>.from(Platform.environment);

    // Fallback: if OPENSSL_DIR/OPENSSL_INCLUDE_DIR/OPENSSL_LIB_DIR are not set
    // but we're in a nix shell, extract openssl paths from NIX_CFLAGS_COMPILE
    // and NIX_LDFLAGS (which nix always populates for packages in buildInputs).
    if (!env.containsKey('OPENSSL_DIR') &&
        !env.containsKey('OPENSSL_INCLUDE_DIR')) {
      final cflags = env['NIX_CFLAGS_COMPILE'] ?? '';
      final ldflags = env['NIX_LDFLAGS'] ?? '';

      final includeMatch =
          RegExp(r'-isystem\s+(\S*openssl[^/]*/include)').firstMatch(cflags);
      final libMatch =
          RegExp(r'-L(\S*openssl[^/]*/lib)').firstMatch(ldflags);

      if (includeMatch != null) {
        env['OPENSSL_INCLUDE_DIR'] = includeMatch.group(1)!;
      }
      if (libMatch != null) {
        env['OPENSSL_LIB_DIR'] = libMatch.group(1)!;
      }
    }

    final builder = RustBuilder(
      assetName: 'uniffi:cdk',
      extraCargoEnvironmentVariables: env,
    );
    await builder.run(input: input, output: output);
  });
}

String _targetTriple(CodeConfig config) {
  return switch ((config.targetOS, config.targetArchitecture)) {
    (OS.android, Architecture.arm64) => 'aarch64-linux-android',
    (OS.android, Architecture.arm) => 'armv7-linux-androideabi',
    (OS.android, Architecture.x64) => 'x86_64-linux-android',
    (OS.iOS, Architecture.arm64) => 'aarch64-apple-ios',
    (OS.windows, Architecture.x64) => 'x86_64-pc-windows-msvc',
    (OS.linux, Architecture.arm64) => 'aarch64-unknown-linux-gnu',
    (OS.linux, Architecture.x64) => 'x86_64-unknown-linux-gnu',
    (OS.macOS, Architecture.arm64) => 'aarch64-apple-darwin',
    (OS.macOS, Architecture.x64) => 'x86_64-apple-darwin',
    _ => throw UnsupportedError(
        'Unsupported target: ${config.targetOS} / ${config.targetArchitecture}'),
  };
}

LinkMode _linkMode(CodeConfig config) {
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
