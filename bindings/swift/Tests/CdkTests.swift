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
