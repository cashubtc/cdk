package org.cashudevkit

import java.io.File
import java.util.concurrent.atomic.AtomicInteger

private class PreparationDatabase(private val native: WalletDatabase) : WalletDatabase by native {
    var reject = true
    val calls = AtomicInteger()

    override suspend fun prepareOperation(preparationJson: String) {
        calls.incrementAndGet()
        if (reject) throw FfiException.Internal("injected preparation failure")
        native.prepareOperation(preparationJson)
    }

    override suspend fun reserveProofs(ys: List<PublicKey>, operationId: String) {
        error("Preparation must use the atomic callback")
    }

    override suspend fun reserveMeltQuote(quoteId: String, operationId: String) {
        error("Preparation must use the atomic callback")
    }

    override suspend fun addSaga(sagaJson: String) {
        error("Preparation must use the atomic callback")
    }
}

suspend fun runPreparationBindingRegression(fixtures: File) {
    for (method in listOf("bolt11", "bolt12", "onchain")) {
        val path = File.createTempFile("cdk_preparation_", ".sqlite")
        val native = WalletSqliteDatabase(path.absolutePath)
        val db = PreparationDatabase(native)
        val proof = decodeProofInfo(File(fixtures, "preparation-proof.json").readText())
        val quote = decodeMeltQuote(File(fixtures, "preparation-quote.json").readText().replace("bolt11", method))
        val keyset = decodeKeySetInfo(File(fixtures, "preparation-keyset.json").readText())
        native.addMint(proof.mintUrl, null)
        native.addMintKeysets(proof.mintUrl, listOf(keyset))
        native.updateProofs(listOf(proof), emptyList())
        native.addMeltQuote(quote)
        val wallet = Wallet("https://preparation.example.com", CurrencyUnit.Sat,
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
            customWalletStore(db), WalletConfig(targetProofCount = null))
        try {
            check(runCatching { wallet.prepareMeltProofs(quote.id, listOf(proof.proof)) }.isFailure)
            check(db.calls.get() == 1)
            WalletSqliteDatabase(path.absolutePath).use { reopened ->
                val available = reopened.getProofsByYs(listOf(proof.y)).single()
                check(available.state == ProofState.UNSPENT && available.usedByOperation == null)
                check(reopened.getMeltQuote(quote.id)?.usedByOperation == null)
                check(reopened.getIncompleteSagas().isEmpty())
                db.reject = false
                val prepared = wallet.prepareMeltProofs(quote.id, listOf(proof.proof))
                check(db.calls.get() == 2)
                prepared.cancel()
                wallet.recoverIncompleteSagas()
                wallet.recoverIncompleteSagas()
                val released = reopened.getProofsByYs(listOf(proof.y)).single()
                check(released.state == ProofState.UNSPENT && released.usedByOperation == null)
            }
        } finally { wallet.close(); native.close(); path.delete() }
    }
}
