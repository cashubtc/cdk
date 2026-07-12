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
