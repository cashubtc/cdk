import 'dart:io';
import 'package:test/test.dart';
import 'package:cdk/cdk.dart';

void main() {
  late Wallet wallet;
  late String dbPath;

  final String mintUrl =
      Platform.environment['CDK_DART_TEST_MINT_URL'] ??
      'https://dummy-mint-url-for-local-testing.invalid';
  final bool runLiveMintTests =
      Platform.environment.containsKey('CDK_DART_TEST_MINT_URL') &&
      Platform.environment['CDK_DART_TEST_MINT_URL']!.isNotEmpty;
  final int settlementDelaySeconds =
      int.tryParse(
        Platform.environment['CDK_DART_MINT_SETTLEMENT_DELAY_SECONDS'] ?? '',
      ) ??
      3;

  setUp(() {
    final tempDir = Directory.systemTemp;
    dbPath = '${tempDir.path}/${DateTime.now().microsecondsSinceEpoch}.sqlite';
    wallet = Wallet(
      mintUrl: mintUrl,
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

  test('in-memory sqlite handles concurrent access', () async {
    final memoryWallet = Wallet(
      mintUrl: mintUrl,
      unit: SatCurrencyUnit(),
      mnemonic: generateMnemonic(),
      store: SqliteWalletStore(':memory:'),
      config: WalletConfig(targetProofCount: null),
    );

    try {
      final balances = await Future.wait(
        List.generate(64, (_) => memoryWallet.totalBalance()),
      );

      for (final balance in balances) {
        expect(balance.value, equals(0));
      }
    } finally {
      memoryWallet.dispose();
    }
  });

  Wallet rateLimitedWallet(RateLimit? rateLimit) => Wallet(
    mintUrl: 'https://mint.example.com',
    unit: SatCurrencyUnit(),
    mnemonic: generateMnemonic(),
    store: SqliteWalletStore(':memory:'),
    config: WalletConfig(targetProofCount: null, rateLimit: rateLimit),
  );

  test('rate limit defaults to pacing', () {
    // Omitting the field is the backwards-compatible spelling and must keep
    // selecting the built-in default.
    final omitted = Wallet(
      mintUrl: 'https://mint.example.com',
      unit: SatCurrencyUnit(),
      mnemonic: generateMnemonic(),
      store: SqliteWalletStore(':memory:'),
      config: WalletConfig(targetProofCount: null),
    );
    try {
      expect(omitted.isRateLimited(), isTrue);
    } finally {
      omitted.dispose();
    }

    final explicit = rateLimitedWallet(DefaultRateLimit());
    try {
      expect(explicit.isRateLimited(), isTrue);
    } finally {
      explicit.dispose();
    }
  });

  test('rate limit can be disabled and re-enabled', () {
    final wallet = rateLimitedWallet(DisabledRateLimit());
    try {
      expect(wallet.isRateLimited(), isFalse);
      wallet.setRateLimit(rateLimit: DefaultRateLimit());
      expect(wallet.isRateLimited(), isTrue);
    } finally {
      wallet.dispose();
    }
  });

  test('custom rate limit paces the wallet', () {
    final wallet = rateLimitedWallet(
      CustomRateLimit(capacity: 5, refillPerMinute: 30),
    );
    try {
      expect(wallet.isRateLimited(), isTrue);
    } finally {
      wallet.dispose();
    }
  });

  test('zero rate limit values are rejected', () {
    expect(
      () => rateLimitedWallet(CustomRateLimit(capacity: 0, refillPerMinute: 30)),
      throwsA(isA<FfiException>()),
    );
    expect(
      () => rateLimitedWallet(CustomRateLimit(capacity: 5, refillPerMinute: 0)),
      throwsA(isA<FfiException>()),
    );
  });

  test(
    'mint flow',
    () async {
      final quote = await wallet.mintQuote(
        paymentMethod: Bolt11PaymentMethod(),
        amount: Amount(value: 100),
        description: null,
        extra: null,
      );

      expect(quote.id, isNotEmpty);
      expect(quote.request, isNotEmpty);

      // testnut pays quotes automatically, wait briefly for payment to settle
      await Future.delayed(Duration(seconds: settlementDelaySeconds));

      final proofs = await wallet.mint(
        quoteId: quote.id,
        amountSplitTarget: NoneSplitTarget(),
        spendingConditions: null,
      );

      expect(proofs, isNotEmpty);

      final balance = await wallet.totalBalance();
      expect(balance.value, equals(100));
    },
    skip: !runLiveMintTests
        ? 'Set CDK_DART_TEST_MINT_URL to run live mint integration tests'
        : false,
  );
}
