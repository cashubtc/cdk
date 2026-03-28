import 'dart:io';
import 'package:test/test.dart';
import 'package:cdk/cdk.dart';

void main() {
  late Wallet wallet;
  late String dbPath;

  setUp(() {
    final tempDir = Directory.systemTemp;
    dbPath = '${tempDir.path}/${DateTime.now().microsecondsSinceEpoch}.sqlite';
    wallet = Wallet(
      mintUrl: 'https://testnut.cashudevkit.org',
      unit: SatCurrencyUnit(),
      mnemonic: generateMnemonic(),
      store: SqliteWalletStore(dbPath),
      config: WalletConfig(targetProofCount: null),
    );
  });

  tearDown(() {
    wallet.dispose();
    try {
      File(dbPath).deleteSync();
    } catch (_) {}
  });

  test('initial balance is zero', () async {
    final balance = await wallet.totalBalance();
    expect(balance.value, equals(0));
  });

  test('mint flow', () async {
    final quote = await wallet.mintQuote(
      paymentMethod: Bolt11PaymentMethod(),
      amount: Amount(value: 100),
      description: null,
      extra: null,
    );

    expect(quote.id, isNotEmpty);
    expect(quote.request, isNotEmpty);

    // testnut pays quotes automatically, wait briefly for payment to settle
    await Future.delayed(Duration(seconds: 3));

    final proofs = await wallet.mint(
      quoteId: quote.id,
      amountSplitTarget: NoneSplitTarget(),
      spendingConditions: null,
    );

    expect(proofs, isNotEmpty);

    final balance = await wallet.totalBalance();
    expect(balance.value, equals(100));
  });
}
