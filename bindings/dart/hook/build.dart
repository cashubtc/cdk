import 'dart:io';

import 'package:hooks/hooks.dart';
import 'package:native_toolchain_rust/native_toolchain_rust.dart';

void main(List<String> args) async {
  await build(args, (input, output) async {
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
