import 'dart:io';

import 'package:code_assets/code_assets.dart';
import 'package:hooks/hooks.dart';
import 'package:native_toolchain_rust/native_toolchain_rust.dart';
import 'package:path/path.dart' as p;

import 'target.dart';

void main(List<String> args) async {
  await build(args, (input, output) async {
    if (!input.config.buildCodeAssets) return;

    final codeConfig = input.config.code;
    final triple = targetTriple(codeConfig);
    final linkMode = linkModeFor(codeConfig);
    final packageRoot = p.fromUri(input.packageRoot);
    final libFileName =
        codeConfig.targetOS.libraryFileName('cdk_ffi_dart', linkMode);
    final prebuiltPath =
        p.join(packageRoot, 'prebuilt', triple, libFileName);

    if (File(prebuiltPath).existsSync()) {
      // Pre-built binary found, so skip cargo entirely.
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

    // Only a monorepo checkout ships the Rust crate; the published package does
    // not. Its absence is terminal, so name what was missing rather than
    // failing somewhere deeper in cargo.
    if (!File(p.join(packageRoot, 'rust', 'Cargo.toml')).existsSync()) {
      throw StateError(
        'No prebuilt library at prebuilt/$triple/$libFileName, and this '
        'package ships no Rust sources to build one from.',
      );
    }

    // No pre-built binary, so build from source via cargo.
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
