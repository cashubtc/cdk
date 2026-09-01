import 'dart:io';

import 'package:cdk/cdk.dart';
import 'package:test/test.dart';

void main() {
  late Wallet wallet;
  late String dbPath;

  final mintUrl =
      Platform.environment['CDK_DART_TEST_MINT_URL'] ??
      'https://dummy-mint-url-for-local-testing.invalid';
  final runLiveMintTests =
      Platform.environment['CDK_DART_TEST_MINT_URL']?.isNotEmpty ?? false;
  final settlementDelaySeconds =
      int.tryParse(
        Platform.environment['CDK_DART_MINT_SETTLEMENT_DELAY_SECONDS'] ?? '',
      ) ??
      3;

  setUp(() {
    dbPath =
        '${Directory.systemTemp.path}/${DateTime.now().microsecondsSinceEpoch}.sqlite';
    wallet = Wallet.open(
      request: WalletOpenRequest(
        mintUrl: mintUrl,
        unit: SatCurrencyUnit(),
        mnemonic: generateMnemonic(),
        store: SqliteWalletStore(dbPath),
        config: WalletConfig(targetProofCount: null),
      ),
    );
  });

  tearDown(() {
    wallet.dispose();
    try {
      File(dbPath).deleteSync();
    } catch (_) {}
  });

  test('initial balance is zero', () async {
    final balance = await wallet.balance();
    expect(balance.available.value, equals(0));
    expect(balance.pending.value, equals(0));
    expect(balance.reserved.value, equals(0));
  });

  test('in-memory sqlite handles concurrent access', () async {
    final memoryWallet = Wallet.open(
      request: WalletOpenRequest(
        mintUrl: mintUrl,
        unit: SatCurrencyUnit(),
        mnemonic: generateMnemonic(),
        store: SqliteWalletStore(':memory:'),
        config: WalletConfig(targetProofCount: null),
      ),
    );

    try {
      final balances = await Future.wait(
        List.generate(64, (_) => memoryWallet.balance()),
      );
      for (final balance in balances) {
        expect(balance.available.value, equals(0));
      }
    } finally {
      memoryWallet.dispose();
    }
  });

  Wallet configuredWallet(RateLimit? rateLimit) => Wallet.open(
    request: WalletOpenRequest(
      mintUrl: 'https://mint.example.com',
      unit: SatCurrencyUnit(),
      mnemonic: generateMnemonic(),
      store: SqliteWalletStore(':memory:'),
      config: WalletConfig(
        targetProofCount: null,
        rateLimit: rateLimit,
      ),
    ),
  );

  test('construction accepts supported pacing policies', () {
    for (final policy in <RateLimit>[
      DefaultRateLimit(),
      DisabledRateLimit(),
      CustomRateLimit(capacity: 5, refillPerMinute: 30),
    ]) {
      final opened = configuredWallet(policy);
      expect(opened.identity().mintUrl.url, 'https://mint.example.com');
      opened.dispose();
    }
  });

  test('zero rate limit values are rejected', () {
    expect(
      () => configuredWallet(
        CustomRateLimit(capacity: 0, refillPerMinute: 30),
      ),
      throwsA(isA<FfiException>()),
    );
    expect(
      () => configuredWallet(
        CustomRateLimit(capacity: 5, refillPerMinute: 0),
      ),
      throwsA(isA<FfiException>()),
    );
  });

  test(
    'mint flow',
    () async {
      final session = await wallet.requestMinting(
        request: MintRequest(
          method: Bolt11PaymentMethod(),
          amount: Amount(value: 100),
        ),
      );
      try {
        final initial = session.initialState();
        expect(initial.id, isNotEmpty);
        expect(initial.paymentRequest, isNotEmpty);

        // testnut pays quotes automatically.
        await Future.delayed(Duration(seconds: settlementDelaySeconds));
        await session.refresh();
        final claimed = await session.claim();
        expect(claimed.value, equals(100));

        final balance = await wallet.balance();
        expect(balance.available.value, equals(100));
      } finally {
        session.dispose();
      }
    },
    skip: !runLiveMintTests
        ? 'Set CDK_DART_TEST_MINT_URL to run live mint integration tests'
        : false,
  );
}
