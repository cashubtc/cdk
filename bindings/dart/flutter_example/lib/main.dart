import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:cdk/cdk.dart';

void main() {
  runApp(const WalletApp());
}

class WalletApp extends StatelessWidget {
  const WalletApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'CDK Wallet',
      theme: ThemeData(
        colorSchemeSeed: Colors.deepPurple,
        useMaterial3: true,
        brightness: Brightness.dark,
      ),
      home: const WalletHome(),
    );
  }
}

class WalletHome extends StatefulWidget {
  const WalletHome({super.key});

  @override
  State<WalletHome> createState() => _WalletHomeState();
}

class _WalletHomeState extends State<WalletHome> {
  Wallet? _wallet;
  int _balance = 0;
  String _status = '';
  bool _loading = false;
  final _mintUrlController =
      TextEditingController(text: 'https://testnut.cashu.space');
  final _amountController = TextEditingController();
  final _tokenController = TextEditingController();
  MintQuote? _activeQuote;
  String? _lastTokenSent;

  @override
  void initState() {
    super.initState();
    _initWallet();
  }

  Future<void> _initWallet() async {
    setState(() => _loading = true);
    try {
      final dbPath =
          '${Directory.systemTemp.path}/cdk_example_wallet.db';
      final mnemonic = generateMnemonic();
      _wallet = Wallet(
        mintUrl: _mintUrlController.text,
        unit: SatCurrencyUnit(),
        mnemonic: mnemonic,
        store: SqliteWalletStore(dbPath),
        config: WalletConfig(targetProofCount: null),
      );
      await _refreshBalance();
      _setStatus('Wallet initialized');
    } catch (e, stackTrace) {
      _setStatus('Init error: $e', e, stackTrace);
    } finally {
      setState(() => _loading = false);
    }
  }

  Future<void> _refreshBalance() async {
    if (_wallet == null) return;
    try {
      final balance = await _wallet!.totalBalance();
      setState(() => _balance = balance.value);
    } catch (e, stackTrace) {
      _setStatus('Balance error: $e', e, stackTrace);
    }
  }

  void _setStatus(String status, [Object? error, StackTrace? stackTrace]) {
    debugPrint('CDK: $status');
    if (error != null) {
      debugPrint('CDK error: $error');
    }
    if (stackTrace != null) {
      debugPrint('$stackTrace');
    }
    setState(() => _status = status);
  }

  // ── Mint (receive via Lightning) ──

  Future<void> _createMintQuote() async {
    if (_wallet == null) return;
    final amountStr = _amountController.text.trim();
    if (amountStr.isEmpty) {
      _setStatus('Enter an amount');
      return;
    }
    final amount = int.tryParse(amountStr);
    if (amount == null || amount <= 0) {
      _setStatus('Invalid amount');
      return;
    }

    setState(() => _loading = true);
    try {
      final quote = await _wallet!.mintQuote(
        paymentMethod: Bolt11PaymentMethod(),
        amount: Amount(value: amount),
        description: null,
        extra: null,
      );
      setState(() => _activeQuote = quote);
      _setStatus('Invoice created — pay it, then tap "Mint tokens"');
    } catch (e, stackTrace) {
      _setStatus('Mint quote error: $e', e, stackTrace);
    } finally {
      setState(() => _loading = false);
    }
  }

  Future<void> _mintTokens() async {
    if (_wallet == null || _activeQuote == null) return;
    setState(() => _loading = true);
    try {
      await _wallet!.mint(quoteId: _activeQuote!.id, amountSplitTarget: NoneSplitTarget(), spendingConditions: null);
      await _refreshBalance();
      setState(() => _activeQuote = null);
      _setStatus('Tokens minted!');
    } catch (e, stackTrace) {
      _setStatus('Mint error: $e', e, stackTrace);
    } finally {
      setState(() => _loading = false);
    }
  }

  // ── Send (create token) ──

  Future<void> _sendTokens() async {
    if (_wallet == null) return;
    final amountStr = _amountController.text.trim();
    if (amountStr.isEmpty) {
      _setStatus('Enter an amount');
      return;
    }
    final amount = int.tryParse(amountStr);
    if (amount == null || amount <= 0) {
      _setStatus('Invalid amount');
      return;
    }

    setState(() => _loading = true);
    try {
      final opts = SendOptions(
        memo: null,
        conditions: null,
        amountSplitTarget: NoneSplitTarget(),
        sendKind: OnlineExactSendKind(),
        includeFee: false,
        maxProofs: null,
        metadata: {},
      );
      final prepared = await _wallet!.prepareSend(amount: Amount(value: amount), options: opts);
      final token = await prepared.confirm(memo: null);
      final encoded = token.encode();
      setState(() => _lastTokenSent = encoded);
      await _refreshBalance();
      _setStatus('Token created — copy it below');
    } catch (e, stackTrace) {
      _setStatus('Send error: $e', e, stackTrace);
    } finally {
      setState(() => _loading = false);
    }
  }

  // ── Receive token ──

  Future<void> _receiveToken() async {
    if (_wallet == null) return;
    final tokenStr = _tokenController.text.trim();
    if (tokenStr.isEmpty) {
      _setStatus('Paste a Cashu token');
      return;
    }

    setState(() => _loading = true);
    try {
      final token = Token.decode(encodedToken: tokenStr);
      final opts = ReceiveOptions(amountSplitTarget: NoneSplitTarget(), p2pkSigningKeys: [], preimages: [], metadata: {});
      final received = await _wallet!.receive(token: token, options: opts);
      await _refreshBalance();
      _tokenController.clear();
      _setStatus('Received ${received.value} sats');
    } catch (e, stackTrace) {
      _setStatus('Receive error: $e', e, stackTrace);
    } finally {
      setState(() => _loading = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    return DefaultTabController(
      length: 3,
      child: Scaffold(
        appBar: AppBar(
          title: const Text('CDK Wallet'),
          bottom: const TabBar(
            tabs: [
              Tab(icon: Icon(Icons.download), text: 'Receive LN'),
              Tab(icon: Icon(Icons.upload), text: 'Send'),
              Tab(icon: Icon(Icons.qr_code), text: 'Receive Token'),
            ],
          ),
        ),
        body: Column(
          children: [
            // Balance header
            Container(
              width: double.infinity,
              padding: const EdgeInsets.all(24),
              color: Theme.of(context).colorScheme.primaryContainer,
              child: Column(
                children: [
                  Text(
                    '$_balance sats',
                    style: Theme.of(context).textTheme.headlineLarge,
                  ),
                  const SizedBox(height: 4),
                  Text(
                    _mintUrlController.text,
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                ],
              ),
            ),

            // Status bar
            if (_status.isNotEmpty)
              Container(
                width: double.infinity,
                padding:
                    const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
                color: Theme.of(context).colorScheme.surfaceContainerHighest,
                child: Text(_status, style: const TextStyle(fontSize: 13)),
              ),

            if (_loading) const LinearProgressIndicator(),

            // Tab content
            Expanded(
              child: TabBarView(
                children: [
                  _buildReceiveLnTab(),
                  _buildSendTab(),
                  _buildReceiveTokenTab(),
                ],
              ),
            ),
          ],
        ),
        floatingActionButton: FloatingActionButton(
          onPressed: _refreshBalance,
          tooltip: 'Refresh balance',
          child: const Icon(Icons.refresh),
        ),
      ),
    );
  }

  Widget _buildReceiveLnTab() {
    return SingleChildScrollView(
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          TextField(
            controller: _amountController,
            decoration: const InputDecoration(
              labelText: 'Amount (sats)',
              border: OutlineInputBorder(),
            ),
            keyboardType: TextInputType.number,
          ),
          const SizedBox(height: 12),
          ElevatedButton.icon(
            onPressed: _loading ? null : _createMintQuote,
            icon: const Icon(Icons.bolt),
            label: const Text('Create Lightning Invoice'),
          ),
          if (_activeQuote != null) ...[
            const SizedBox(height: 16),
            const Text('Lightning Invoice:',
                style: TextStyle(fontWeight: FontWeight.bold)),
            const SizedBox(height: 8),
            SelectableText(
              _activeQuote!.request,
              style: const TextStyle(fontSize: 12, fontFamily: 'monospace'),
            ),
            const SizedBox(height: 8),
            Row(
              children: [
                ElevatedButton.icon(
                  onPressed: () {
                    Clipboard.setData(
                        ClipboardData(text: _activeQuote!.request));
                    _setStatus('Invoice copied');
                  },
                  icon: const Icon(Icons.copy),
                  label: const Text('Copy'),
                ),
                const SizedBox(width: 12),
                FilledButton.icon(
                  onPressed: _loading ? null : _mintTokens,
                  icon: const Icon(Icons.check),
                  label: const Text('Mint Tokens'),
                ),
              ],
            ),
          ],
        ],
      ),
    );
  }

  Widget _buildSendTab() {
    return SingleChildScrollView(
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          TextField(
            controller: _amountController,
            decoration: const InputDecoration(
              labelText: 'Amount (sats)',
              border: OutlineInputBorder(),
            ),
            keyboardType: TextInputType.number,
          ),
          const SizedBox(height: 12),
          ElevatedButton.icon(
            onPressed: _loading ? null : _sendTokens,
            icon: const Icon(Icons.send),
            label: const Text('Create Cashu Token'),
          ),
          if (_lastTokenSent != null) ...[
            const SizedBox(height: 16),
            const Text('Cashu Token:',
                style: TextStyle(fontWeight: FontWeight.bold)),
            const SizedBox(height: 8),
            SelectableText(
              _lastTokenSent!,
              style: const TextStyle(fontSize: 12, fontFamily: 'monospace'),
              maxLines: 6,
            ),
            const SizedBox(height: 8),
            ElevatedButton.icon(
              onPressed: () {
                Clipboard.setData(ClipboardData(text: _lastTokenSent!));
                _setStatus('Token copied');
              },
              icon: const Icon(Icons.copy),
              label: const Text('Copy Token'),
            ),
          ],
        ],
      ),
    );
  }

  Widget _buildReceiveTokenTab() {
    return SingleChildScrollView(
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          TextField(
            controller: _tokenController,
            decoration: const InputDecoration(
              labelText: 'Paste Cashu token',
              border: OutlineInputBorder(),
            ),
            maxLines: 4,
          ),
          const SizedBox(height: 12),
          ElevatedButton.icon(
            onPressed: _loading ? null : _receiveToken,
            icon: const Icon(Icons.redeem),
            label: const Text('Receive Token'),
          ),
        ],
      ),
    );
  }
}
