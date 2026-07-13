import Testing
import Foundation
@testable import Cdk

@Suite("Cdk Wallet Tests")
struct CdkTests {
    private let wallet: Wallet
    private let dbPath: String

    init() async throws {
        let tempDir = FileManager.default.temporaryDirectory
        dbPath = tempDir.appendingPathComponent(UUID().uuidString + ".sqlite").path
        wallet = try Wallet(
            mintUrl: "https://testnut.cashudevkit.org",
            unit: .sat,
            mnemonic: try generateMnemonic(),
            store: .sqlite(path: dbPath),
            config: WalletConfig(targetProofCount: nil)
        )
    }

    @Test("Initial balance is zero")
    func initialBalanceIsZero() async throws {
        let balance = try await wallet.totalBalance()
        #expect(balance.value == 0, "New wallet should have zero balance")
    }

    @Test("In-memory SQLite handles concurrent access")
    func inMemorySqliteHandlesConcurrentAccess() async throws {
        let memoryWallet = try Wallet(
            mintUrl: "https://testnut.cashudevkit.org",
            unit: .sat,
            mnemonic: try generateMnemonic(),
            store: .sqlite(path: ":memory:"),
            config: WalletConfig(targetProofCount: nil)
        )

        let balances = try await withThrowingTaskGroup(of: Amount.self) { group in
            for _ in 0..<64 {
                group.addTask {
                    try await memoryWallet.totalBalance()
                }
            }

            var balances: [Amount] = []
            for try await balance in group {
                balances.append(balance)
            }
            return balances
        }

        #expect(balances.count == 64, "All concurrent balance reads should complete")
        for balance in balances {
            #expect(balance.value == 0, "New in-memory wallet should have zero balance")
        }
    }

    private func rateLimitedWallet(_ rateLimit: RateLimit?) throws -> Wallet {
        try Wallet(
            mintUrl: "https://mint.example.com",
            unit: .sat,
            mnemonic: try generateMnemonic(),
            store: .sqlite(path: ":memory:"),
            config: WalletConfig(targetProofCount: nil, rateLimit: rateLimit)
        )
    }

    @Test("Rate limit defaults to pacing")
    func rateLimitDefaultsToPacing() throws {
        // Omitting the field is the backwards-compatible spelling and must keep
        // selecting the built-in default.
        let omitted = try Wallet(
            mintUrl: "https://mint.example.com",
            unit: .sat,
            mnemonic: try generateMnemonic(),
            store: .sqlite(path: ":memory:"),
            config: WalletConfig(targetProofCount: nil)
        )
        #expect(omitted.isRateLimited(), "An omitted rate limit should pace by default")

        let explicit = try rateLimitedWallet(.default)
        #expect(explicit.isRateLimited(), "Default should pace the wallet")
    }

    @Test("Rate limit can be disabled and re-enabled")
    func rateLimitCanBeDisabledAndReEnabled() throws {
        let wallet = try rateLimitedWallet(.disabled)
        #expect(!wallet.isRateLimited(), "Disabled should not pace")

        try wallet.setRateLimit(rateLimit: .default)
        #expect(wallet.isRateLimited(), "A wallet built disabled should be re-enablable")
    }

    @Test("Custom rate limit paces the wallet")
    func customRateLimitPaces() throws {
        let wallet = try rateLimitedWallet(.custom(capacity: 5, refillPerMinute: 30))
        #expect(wallet.isRateLimited(), "Custom should pace the wallet")
    }

    @Test("Zero rate limit values are rejected")
    func zeroRateLimitValuesAreRejected() throws {
        #expect(throws: (any Error).self) {
            try rateLimitedWallet(.custom(capacity: 0, refillPerMinute: 30))
        }
        #expect(throws: (any Error).self) {
            try rateLimitedWallet(.custom(capacity: 5, refillPerMinute: 0))
        }
    }

    @Test("Mint flow completes successfully")
    func mintFlow() async throws {
        let quote = try await wallet.mintQuote(
            paymentMethod: .bolt11,
            amount: Amount(value: 100),
            description: nil,
            extra: nil
        )

        #expect(!quote.id.isEmpty, "Quote should have a non-empty id")
        #expect(!quote.request.isEmpty, "Quote should have a non-empty payment request")

        // testnut pays quotes automatically, wait briefly for payment to settle
        try await Task.sleep(nanoseconds: 3_000_000_000)

        let proofs = try await wallet.mint(
            quoteId: quote.id,
            amountSplitTarget: .none,
            spendingConditions: nil
        )

        #expect(!proofs.isEmpty, "Should have received proofs")

        let balance = try await wallet.totalBalance()
        #expect(balance.value == 100, "Balance should be 100 after minting")
    }
}
