package org.cashudevkit

import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.AfterEach
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertThrows
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.BeforeEach
import org.junit.jupiter.api.Test
import java.io.File

class WalletTest {

    private lateinit var wallet: Wallet
    private lateinit var dbFile: File

    @BeforeEach
    fun setUp() {
        dbFile = File.createTempFile("cdk_test_", ".sqlite")
        wallet = Wallet.open(
            WalletOpenRequest(
                mintUrl = "https://testnut.cashudevkit.org",
                unit = CurrencyUnit.Sat,
                mnemonic = generateMnemonic(),
                store = WalletStore.Sqlite(path = dbFile.absolutePath),
                config = WalletConfig(targetProofCount = null),
            ),
        )
    }

    @AfterEach
    fun tearDown() {
        wallet.close()
        dbFile.delete()
    }

    @Test
    fun `initial balance is zero`() = runBlocking {
        val balance = wallet.balance()
        assertEquals(0UL, balance.available.value)
        assertEquals(0UL, balance.pending.value)
        assertEquals(0UL, balance.reserved.value)
    }

    @Test
    fun `in-memory sqlite handles concurrent access`() = runBlocking {
        val memoryWallet = Wallet.open(
            WalletOpenRequest(
                mintUrl = "https://testnut.cashudevkit.org",
                unit = CurrencyUnit.Sat,
                mnemonic = generateMnemonic(),
                store = WalletStore.Sqlite(path = ":memory:"),
                config = WalletConfig(targetProofCount = null),
            ),
        )

        try {
            val balances = coroutineScope {
                (0 until 64).map {
                    async { memoryWallet.balance() }
                }.awaitAll()
            }
            balances.forEach { balance ->
                assertEquals(0UL, balance.available.value)
            }
        } finally {
            memoryWallet.close()
        }
    }

    private fun configuredWallet(rateLimit: RateLimit?): Wallet = Wallet.open(
        WalletOpenRequest(
            mintUrl = "https://mint.example.com",
            unit = CurrencyUnit.Sat,
            mnemonic = generateMnemonic(),
            store = WalletStore.Sqlite(path = ":memory:"),
            config = WalletConfig(targetProofCount = null, rateLimit = rateLimit),
        ),
    )

    @Test
    fun `construction accepts supported pacing policies`() {
        listOf(
            RateLimit.Default,
            RateLimit.Disabled,
            RateLimit.Custom(capacity = 5U, refillPerMinute = 30U),
        ).forEach { policy ->
            configuredWallet(policy).use { opened ->
                assertEquals("https://mint.example.com", opened.identity().mintUrl.url)
            }
        }
    }

    @Test
    fun `zero rate limit values are rejected`() {
        assertThrows(FfiException::class.java) {
            configuredWallet(RateLimit.Custom(capacity = 0U, refillPerMinute = 30U))
        }
        assertThrows(FfiException::class.java) {
            configuredWallet(RateLimit.Custom(capacity = 5U, refillPerMinute = 0U))
        }
    }

    @Test
    fun `mint flow`() = runBlocking {
        val session = wallet.requestMinting(
            MintRequest(
                method = PaymentMethod.Bolt11,
                amount = Amount(value = 100UL),
            ),
        )
        try {
            val initial = session.initialState()
            assertTrue(initial.id.isNotEmpty())
            assertTrue(initial.paymentRequest.isNotEmpty())

            // testnut pays quotes automatically.
            delay(3000)
            session.refresh()
            val claimed = session.claim()
            assertEquals(100UL, claimed.value)

            val balance = wallet.balance()
            assertEquals(100UL, balance.available.value)
        } finally {
            session.close()
        }
    }
}
