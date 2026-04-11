/**
 * CDK React Native Bindings — Integration Tests
 *
 * These tests mirror the Dart binding tests (bindings/dart/test/wallet_test.dart)
 * and Swift binding tests (bindings/swift/Tests/CdkTests.swift).
 *
 * They exercise the actual Rust FFI via a napi-rs Node.js native addon
 * (node-addon/) that wraps the same cdk-ffi crate used by all language bindings.
 *
 * Test cases:
 *   1. Create a Wallet with SQLite store
 *   2. Verify initial balance is zero
 *   3. Full mint flow: mintQuote → wait → mint → verify balance
 */

import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';

// The native addon is built by napi-rs into node-addon/
// eslint-disable-next-line @typescript-eslint/no-var-requires
const { Wallet, generateMnemonic } = require('../node-addon');

describe('Wallet', () => {
  let wallet: any;
  let dbPath: string;

  beforeEach(() => {
    dbPath = path.join(os.tmpdir(), `${Date.now()}-${Math.random().toString(36).slice(2)}.sqlite`);
    const mnemonic = generateMnemonic();
    wallet = new Wallet(
      'https://testnut.cashudevkit.org',
      'sat',
      mnemonic,
      dbPath,
      undefined, // targetProofCount
    );
  });

  afterEach(() => {
    wallet = null;
    try {
      fs.unlinkSync(dbPath);
    } catch {
      // ignore
    }
  });

  test('initial balance is zero', async () => {
    const balance = await wallet.totalBalance();
    expect(balance).toBe(0);
  });

  test('mint flow', async () => {
    const quote = await wallet.mintQuote('bolt11', 100, null, null);

    expect(quote.id).toBeTruthy();
    expect(quote.request).toBeTruthy();

    // testnut pays quotes automatically, wait briefly for payment to settle
    await new Promise((r) => setTimeout(r, 3000));

    const proofs = await wallet.mint(quote.id, 'none', null);

    expect(proofs.length).toBeGreaterThan(0);

    const balance = await wallet.totalBalance();
    expect(balance).toBe(100);
  }, 15000);
});
