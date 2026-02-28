import 'package:hooks/hooks.dart';
import 'package:native_toolchain_rust/native_toolchain_rust.dart';

void main(List<String> args) async {
  await build(args, (input, output) async {
    final builder = RustBuilder(
      assetName: 'uniffi:cdk',
    );
    await builder.run(input: input, output: output);
  });
}
