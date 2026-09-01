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

    @Test("Typed Nostr identity, signing, NIP-44, and npub.cash share one key")
    func typedNostrIdentity() throws {
        let mnemonic = "leader monkey parrot ring guide accident before fence cannon height naive bean"
        let expectedSecret = "7f7ff03d123792d6ac594bfa67bf6d0c0ab55b6b1fdb6249303fe861f1ccba9a"
        let signer = try NostrSigner.fromMnemonic(mnemonic: mnemonic, passphrase: nil)

        #expect(signer.secretKeyHex() == expectedSecret)
        #expect(signer.publicKeyHex().count == 64)
        #expect(signer.xOnlyPublicKeyHex() == signer.publicKeyHex())
        #expect(signer.cashuP2pkPublicKey() == "02" + signer.publicKeyHex())
        #expect(try NostrSigner.fromNsec(nsec: signer.nsec()).publicKeyHex() == signer.publicKeyHex())

        let signed = try signer.signEvent(event: NostrUnsignedEvent(
            createdAt: 1_700_000_000,
            kind: 27_235,
            tags: [
                ["u", "https://example.com/api"],
                ["method", "POST"],
            ],
            content: ""
        ))
        #expect(signed.pubkey == signer.publicKeyHex())
        #expect(signed.id.count == 64)
        #expect(signed.sig.count == 128)

        let peer = NostrSigner.generate()
        let payload = try signer.nip44Encrypt(
            recipientPubkey: peer.publicKeyHex(),
            plaintext: "hello cashu"
        )
        #expect(try peer.nip44Decrypt(
            senderPubkey: signer.publicKeyHex(),
            payload: payload
        ) == "hello cashu")

        let npubCash = NpubCashClient.withSigner(
            baseUrl: "https://npub.cash",
            signer: signer
        )
        #expect(npubCash.identityPubkey() == signer.publicKeyHex())

        let inbox = try NostrInbox.withSigner(
            signer: signer,
            relays: ["wss://relay.example.com"],
            since: 1_700_000_000
        )
        #expect(inbox.pubkey() == signer.publicKeyHex())
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
