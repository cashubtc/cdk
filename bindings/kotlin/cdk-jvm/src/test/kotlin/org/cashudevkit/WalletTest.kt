package org.cashudevkit

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
