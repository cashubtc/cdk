import Foundation
import Testing
@testable import Cdk

@Suite("Cdk Wallet Tests")
struct CdkTests {
    private let wallet: Wallet
    private let dbPath: String

    init() async throws {
        let tempDir = FileManager.default.temporaryDirectory
        dbPath = tempDir.appendingPathComponent(UUID().uuidString + ".sqlite").path
        wallet = try Wallet.open(request: WalletOpenRequest(
            mintUrl: "https://testnut.cashudevkit.org",
            unit: .sat,
            mnemonic: try generateMnemonic(),
            store: .sqlite(path: dbPath),
            config: WalletConfig(targetProofCount: nil)
        ))
    }

    @Test("Initial balance is zero")
    func initialBalanceIsZero() async throws {
        let balance = try await wallet.balance()
        #expect(balance.available.value == 0)
        #expect(balance.pending.value == 0)
        #expect(balance.reserved.value == 0)
    }

    @Test("In-memory SQLite handles concurrent access")
    func inMemorySqliteHandlesConcurrentAccess() async throws {
        let memoryWallet = try Wallet.open(request: WalletOpenRequest(
            mintUrl: "https://testnut.cashudevkit.org",
            unit: .sat,
            mnemonic: try generateMnemonic(),
            store: .sqlite(path: ":memory:"),
            config: WalletConfig(targetProofCount: nil)
        ))

        let balances = try await withThrowingTaskGroup(of: WalletBalance.self) { group in
            for _ in 0..<64 {
                group.addTask {
                    try await memoryWallet.balance()
                }
            }

            var balances: [WalletBalance] = []
            for try await balance in group {
                balances.append(balance)
            }
            return balances
        }

        #expect(balances.count == 64)
        for balance in balances {
            #expect(balance.available.value == 0)
        }
    }

    private func configuredWallet(_ rateLimit: RateLimit?) throws -> Wallet {
        try Wallet.open(request: WalletOpenRequest(
            mintUrl: "https://mint.example.com",
            unit: .sat,
            mnemonic: try generateMnemonic(),
            store: .sqlite(path: ":memory:"),
            config: WalletConfig(targetProofCount: nil, rateLimit: rateLimit)
        ))
    }

    @Test("Construction accepts supported pacing policies")
    func constructionAcceptsPacingPolicies() throws {
        _ = try configuredWallet(.default)
        _ = try configuredWallet(.disabled)
        _ = try configuredWallet(.custom(capacity: 5, refillPerMinute: 30))
    }

    @Test("Zero rate limit values are rejected")
    func zeroRateLimitValuesAreRejected() throws {
        #expect(throws: (any Error).self) {
            try configuredWallet(.custom(capacity: 0, refillPerMinute: 30))
        }
        #expect(throws: (any Error).self) {
            try configuredWallet(.custom(capacity: 5, refillPerMinute: 0))
        }
    }

    @Test("Mint flow completes successfully")
    func mintFlow() async throws {
        let session = try await wallet.requestMinting(request: MintRequest(
            method: .bolt11,
            amount: Amount(value: 100)
        ))
        let initial = session.initialState()
        #expect(!initial.id.isEmpty)
        #expect(!initial.paymentRequest.isEmpty)

        // testnut pays quotes automatically.
        try await Task.sleep(nanoseconds: 3_000_000_000)
        _ = try await session.refresh()
        let claimed = try await session.claim()
        #expect(claimed.value == 100)

        let balance = try await wallet.balance()
        #expect(balance.available.value == 100)
    }
}
