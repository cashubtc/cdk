import Foundation
#if canImport(Testing)
import Testing
#endif
import Cdk

private final class PreparationDatabase: WalletSqliteDatabase, @unchecked Sendable {
    private let lock = NSLock()
    private var reject = true
    private var calls = 0

    func allowPreparation() { lock.withLock { reject = false } }
    var preparationCalls: Int { lock.withLock { calls } }

    override func prepareOperation(preparationJson: String) async throws {
        let rejected = lock.withLock { calls += 1; return reject }
        if rejected { throw FfiError.Internal(errorMessage: "injected preparation failure") }
        try await super.prepareOperation(preparationJson: preparationJson)
    }

    override func reserveProofs(ys: [PublicKey], operationId: String) async throws {
        preconditionFailure("Preparation must use the atomic callback")
    }

    override func reserveMeltQuote(quoteId: String, operationId: String) async throws {
        preconditionFailure("Preparation must use the atomic callback")
    }

    override func addSaga(sagaJson: String) async throws {
        preconditionFailure("Preparation must use the atomic callback")
    }
}

func runPreparationBindingRegression(fixtures: URL) async throws {
    func fixture(_ name: String) throws -> String {
        try String(contentsOf: fixtures.appendingPathComponent("preparation-\(name).json"), encoding: .utf8)
    }
    for method in ["bolt11", "bolt12", "onchain"] {
        let path = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString + ".sqlite").path
        defer { try? FileManager.default.removeItem(atPath: path) }
        let db = try PreparationDatabase(filePath: path)
        let proof = try decodeProofInfo(json: fixture("proof"))
        let quote = try decodeMeltQuote(json: fixture("quote").replacingOccurrences(of: "bolt11", with: method))
        let keyset = try decodeKeySetInfo(json: fixture("keyset"))
        try await db.addMint(mintUrl: proof.mintUrl, mintInfo: nil)
        try await db.addMintKeysets(mintUrl: proof.mintUrl, keysets: [keyset])
        try await db.updateProofs(added: [proof], removedYs: [])
        try await db.addMeltQuote(quote: quote)
        let wallet = try Wallet(mintUrl: "https://preparation.example.com", unit: .sat,
            mnemonic: "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
            store: customWalletStore(db: db), config: WalletConfig(targetProofCount: nil))
        var failed = false
        do { _ = try await wallet.prepareMeltProofs(quoteId: quote.id, proofs: [proof.proof]) }
        catch { failed = true }
        precondition(failed && db.preparationCalls == 1)
        let reopened = try WalletSqliteDatabase(filePath: path)
        let available = try await reopened.getProofsByYs(ys: [proof.y])
        precondition(available.count == 1 && available[0].state == .unspent && available[0].usedByOperation == nil)
        let unowned = try await reopened.getMeltQuote(quoteId: quote.id)
        precondition(unowned?.usedByOperation == nil)
        let sagas = try await reopened.getIncompleteSagas()
        precondition(sagas.isEmpty)
        db.allowPreparation()
        let prepared = try await wallet.prepareMeltProofs(quoteId: quote.id, proofs: [proof.proof])
        precondition(db.preparationCalls == 2)
        try await prepared.cancel()
        _ = try await wallet.recoverIncompleteSagas()
        _ = try await wallet.recoverIncompleteSagas()
        let released = try await reopened.getProofsByYs(ys: [proof.y])
        precondition(released[0].state == .unspent && released[0].usedByOperation == nil)
    }
}

#if canImport(Testing)
@Suite("Wallet preparation storage")
struct PreparationTests {
    @Test("Custom SQLite stores preserve preparation transactions")
    func customStorePreparation() async throws {
        let fixtures = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("test-fixtures")
        try await runPreparationBindingRegression(fixtures: fixtures)
    }
}

#endif
