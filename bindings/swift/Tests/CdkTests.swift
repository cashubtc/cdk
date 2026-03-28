import XCTest
@testable import Cdk

final class CdkTests: XCTestCase {
    private var wallet: Wallet!
    private var dbPath: String!

    override func setUp() async throws {
        try await super.setUp()
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

    override func tearDown() async throws {
        wallet = nil
        if let path = dbPath {
            try? FileManager.default.removeItem(atPath: path)
        }
        try await super.tearDown()
    }

    func testInitialBalanceIsZero() async throws {
        let balance = try await wallet.totalBalance()
        XCTAssertEqual(balance.value, 0, "New wallet should have zero balance")
    }

    func testMintFlow() async throws {
        let quote = try await wallet.mintQuote(
            paymentMethod: .bolt11,
            amount: Amount(value: 100),
            description: nil,
            extra: nil
        )

        XCTAssertFalse(quote.id.isEmpty, "Quote should have a non-empty id")
        XCTAssertFalse(quote.request.isEmpty, "Quote should have a non-empty payment request")

        // testnut pays quotes automatically, so we can mint right away
        let proofs = try await wallet.mint(
            quoteId: quote.id,
            amountSplitTarget: .none,
            spendingConditions: nil
        )

        XCTAssertFalse(proofs.isEmpty, "Should have received proofs")

        let balance = try await wallet.totalBalance()
        XCTAssertEqual(balance.value, 100, "Balance should be 100 after minting")
    }
}
