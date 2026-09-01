package org.cashudevkit

import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.AfterEach
import org.junit.jupiter.api.Assertions.*
import org.junit.jupiter.api.BeforeEach
import org.junit.jupiter.api.Test
import java.io.File

class WalletTest {

    private lateinit var wallet: Wallet
    private lateinit var dbFile: File

    @BeforeEach
    fun setUp() {
        dbFile = File.createTempFile("cdk_test_", ".sqlite")
        val mnemonic = generateMnemonic()
        wallet = Wallet(
            mintUrl = "https://testnut.cashudevkit.org",
            unit = CurrencyUnit.Sat,
            mnemonic = mnemonic,
            store = WalletStore.Sqlite(path = dbFile.absolutePath),
            config = WalletConfig(targetProofCount = null),
        )
    }

    @AfterEach
    fun tearDown() {
        wallet.close()
        dbFile.delete()
    }

    @Test
    fun `initial balance is zero`() = runBlocking {
        val balance = wallet.totalBalance()
        assertEquals(0UL, balance.value)
    }

    @Test
    fun `in-memory sqlite handles concurrent access`() = runBlocking {
        val memoryWallet = Wallet(
            mintUrl = "https://testnut.cashudevkit.org",
            unit = CurrencyUnit.Sat,
            mnemonic = generateMnemonic(),
            store = WalletStore.Sqlite(path = ":memory:"),
            config = WalletConfig(targetProofCount = null),
        )

        try {
            val balances = coroutineScope {
                (0 until 64).map {
                    async { memoryWallet.totalBalance() }
                }.awaitAll()
            }

            balances.forEach { balance ->
                assertEquals(0UL, balance.value)
            }
        } finally {
            memoryWallet.close()
        }
    }

    private fun rateLimitedWallet(rateLimit: RateLimit?): Wallet = Wallet(
        mintUrl = "https://mint.example.com",
        unit = CurrencyUnit.Sat,
        mnemonic = generateMnemonic(),
        store = WalletStore.Sqlite(path = ":memory:"),
        config = WalletConfig(targetProofCount = null, rateLimit = rateLimit),
    )

    @Test
    fun `rate limit defaults to pacing`() {
        // Omitting the field is the backwards-compatible spelling and must keep
        // selecting the built-in default.
        val omitted = Wallet(
            mintUrl = "https://mint.example.com",
            unit = CurrencyUnit.Sat,
            mnemonic = generateMnemonic(),
            store = WalletStore.Sqlite(path = ":memory:"),
            config = WalletConfig(targetProofCount = null),
        )
        try {
            assertTrue(omitted.isRateLimited())
        } finally {
            omitted.close()
        }

        val explicit = rateLimitedWallet(RateLimit.Default)
        try {
            assertTrue(explicit.isRateLimited())
        } finally {
            explicit.close()
        }
    }

    @Test
    fun `rate limit can be disabled and re-enabled`() {
        val disabled = rateLimitedWallet(RateLimit.Disabled)
        try {
            assertFalse(disabled.isRateLimited())
            disabled.setRateLimit(RateLimit.Default)
            assertTrue(disabled.isRateLimited())
        } finally {
            disabled.close()
        }
    }

    @Test
    fun `custom rate limit paces the wallet`() {
        val custom = rateLimitedWallet(RateLimit.Custom(capacity = 5U, refillPerMinute = 30U))
        try {
            assertTrue(custom.isRateLimited())
        } finally {
            custom.close()
        }
    }

    @Test
    fun `zero rate limit values are rejected`() {
        assertThrows(FfiException::class.java) {
            rateLimitedWallet(RateLimit.Custom(capacity = 0U, refillPerMinute = 30U))
        }
        assertThrows(FfiException::class.java) {
            rateLimitedWallet(RateLimit.Custom(capacity = 5U, refillPerMinute = 0U))
        }
    }

    @Test
    fun `typed Nostr identity signing NIP-44 and npubcash share one key`() {
        val mnemonic =
            "leader monkey parrot ring guide accident before fence cannon height naive bean"
        val expectedSecret =
            "7f7ff03d123792d6ac594bfa67bf6d0c0ab55b6b1fdb6249303fe861f1ccba9a"
        val signer = NostrSigner.fromMnemonic(mnemonic, null)
        val peer = NostrSigner.generate()
        val fromNsec = NostrSigner.fromNsec(signer.nsec())
        val npubCash = NpubCashClient.withSigner("https://npub.cash", signer)
        val inbox = NostrInbox.withSigner(
            signer,
            listOf("wss://relay.example.com"),
            1_700_000_000UL,
        )

        try {
            assertEquals(expectedSecret, signer.secretKeyHex())
            assertEquals(64, signer.publicKeyHex().length)
            assertEquals(signer.publicKeyHex(), signer.xOnlyPublicKeyHex())
            assertEquals("02" + signer.publicKeyHex(), signer.cashuP2pkPublicKey())
            assertEquals(signer.publicKeyHex(), fromNsec.publicKeyHex())

            val signed = signer.signEvent(
                NostrUnsignedEvent(
                    createdAt = 1_700_000_000UL,
                    kind = 27_235U.toUShort(),
                    tags = listOf(
                        listOf("u", "https://example.com/api"),
                        listOf("method", "POST"),
                    ),
                    content = "",
                ),
            )
            assertEquals(signer.publicKeyHex(), signed.pubkey)
            assertEquals(64, signed.id.length)
            assertEquals(128, signed.sig.length)

            val payload = signer.nip44Encrypt(peer.publicKeyHex(), "hello cashu")
            assertEquals(
                "hello cashu",
                peer.nip44Decrypt(signer.publicKeyHex(), payload),
            )
            assertEquals(signer.publicKeyHex(), npubCash.identityPubkey())
            assertEquals(signer.publicKeyHex(), inbox.pubkey())
        } finally {
            inbox.close()
            npubCash.close()
            fromNsec.close()
            peer.close()
            signer.close()
        }
    }

    @Test
    fun `mint flow`() = runBlocking {
        val quote = wallet.mintQuote(
            paymentMethod = PaymentMethod.Bolt11,
            amount = Amount(value = 100UL),
            description = null,
            extra = null,
        )

        assertTrue(quote.id.isNotEmpty())
        assertTrue(quote.request.isNotEmpty())

        // testnut pays quotes automatically, wait for payment to settle
        kotlinx.coroutines.delay(3000)

        val proofs = wallet.mint(
            quoteId = quote.id,
            amountSplitTarget = SplitTarget.None,
            spendingConditions = null,
        )

        assertTrue(proofs.isNotEmpty())

        val balance = wallet.totalBalance()
        assertEquals(100UL, balance.value)
    }
}
