# Testing Spark CDK Integration

## Quick Start Guide

### 1. Build CDK with Spark Support

```bash
cd cdk
cargo build --package cdk-mintd --features spark --release
```

### 2. Use the Test Configuration

A complete test configuration has been created: `test-spark-mint.toml`

**⚠️ Important Notes:**
- Uses **Signet network** (safe test network)
- Contains **test mnemonics** (publicly known - only for testing!)
- Pre-configured with sensible defaults
- Swagger UI enabled at `http://127.0.0.1:8085/swagger-ui/`

### 3. Start Your Test Mint

```bash
./target/release/cdk-mintd --config test-spark-mint.toml
```

Expected output:
```
INFO Initializing Spark wallet for network: Signet
INFO Spark wallet initialized successfully
INFO Starting Spark payment processor
INFO Spark payment processor started successfully
INFO Starting server on 127.0.0.1:8085
```

### 4. Test the Mint

#### Option A: Using Swagger UI (Easiest)

1. Open browser: `http://127.0.0.1:8085/swagger-ui/`
2. Try the `/v1/info` endpoint to verify mint is running
3. Create a mint quote with `/v1/mint/quote/bolt11`
4. Pay the invoice from a Signet Lightning wallet
5. Mint ecash with `/v1/mint/bolt11`

#### Option B: Using cURL

**Step 1: Check Mint Info**
```bash
curl http://127.0.0.1:8085/v1/info | jq
```

**Step 2: Create Mint Quote (Get Lightning Invoice)**
```bash
curl -X POST http://127.0.0.1:8085/v1/mint/quote/bolt11 \
  -H "Content-Type: application/json" \
  -d '{"amount": 100, "unit": "sat"}' | jq
```

Response will include:
- `quote`: Quote ID (save this!)
- `request`: Lightning invoice (pay this from a Signet wallet)
- `state`: "UNPAID"

**Step 3: Pay the Invoice**

Use any Signet Lightning wallet to pay the invoice. Options:
- Zeus wallet (Signet mode)
- Phoenix wallet (Signet)
- Lightning CLI tools

**Step 4: Check Quote Status**
```bash
curl http://127.0.0.1:8085/v1/mint/quote/bolt11/<quote_id> | jq
```

Should show `"state": "PAID"` once payment is received.

**Step 5: Mint Ecash Tokens**
```bash
curl -X POST http://127.0.0.1:8085/v1/mint/bolt11 \
  -H "Content-Type: application/json" \
  -d '{
    "quote": "<quote_id>",
    "outputs": [...]
  }' | jq
```

### 5. Getting Signet Test Sats

To test paying invoices (melting), you need Signet sats:

1. **Signet Faucet**: https://signetfaucet.com/
2. **Lightning Faucet**: https://faucet.mutinynet.com/
3. **Exchange**: Use another Signet Lightning wallet

## Test Configuration Details

### Mnemonics in Test Config

The test configuration includes **TWO mnemonics**:

1. **CDK Mint Mnemonic** (`[info].mnemonic`):
   ```
   abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about
   ```
   - Standard test mnemonic (BIP39)
   - Used for signing ecash tokens
   - Safe to use for testing only

2. **Spark Wallet Mnemonic** (`[spark].mnemonic`):
   ```
   economy tooth crop sound merry satisfy album spell iron clean oven worry govern senior whisper venture glide cinnamon muscle left rough budget umbrella fox
   ```
   - Test mnemonic for Spark Lightning wallet
   - Controls Lightning funds
   - Safe to use for testing only

### Network Settings

- **Network**: Signet (safe test network)
- **Listen Port**: 8085
- **Storage**: `./data/spark-test/`
- **Fees**: 1% with 10 sat minimum

## Troubleshooting

### Mint Won't Start

**Check logs for errors:**
```bash
tail -f ./logs/cdk-mintd.log
```

**Common issues:**

1. **Port already in use**
   - Change `listen_port = 8086` in config
   - Or kill process using port 8085

2. **Spark connection failed**
   - Check internet connection
   - Verify Signet network is accessible
   - Wait a moment and retry

3. **Storage directory issues**
   - Ensure write permissions: `chmod 755 ./data/`
   - Try absolute path: `storage_dir = "/full/path/to/data/spark-test"`

### Invoice Not Getting Paid

1. **Check invoice is valid**
   - Verify it's a Signet invoice (starts with `lntbs` for Signet)
   - Check expiry hasn't passed

2. **Check wallet has Signet sats**
   - Verify you're on Signet network
   - Use a Signet faucet to get test sats

3. **Check mint logs**
   - Should see "Received incoming payment event"
   - Check for any errors

### Payment Not Completing

1. **Check Spark wallet connectivity**
   - Logs should show "Spark wallet initialized successfully"
   - Restart mint if needed

2. **Verify payment hash matches**
   - Compare payment hash in invoice to quote

## Testing Checklist

- [ ] Mint starts successfully
- [ ] Can access Swagger UI
- [ ] Can create mint quote
- [ ] Receives Lightning invoice
- [ ] Can pay invoice from external wallet
- [ ] Detects payment (check logs)
- [ ] Can mint ecash tokens
- [ ] Can create melt quote
- [ ] Can pay outgoing Lightning invoice
- [ ] Handles errors gracefully

## Monitoring Your Test Mint

### Check Mint Status
```bash
# Get mint info
curl http://127.0.0.1:8085/v1/info | jq

# Check keysets
curl http://127.0.0.1:8085/v1/keysets | jq

# Get mint keys
curl http://127.0.0.1:8085/v1/keys | jq
```

### View Logs
```bash
# Real-time logs
tail -f ./logs/cdk-mintd.log

# Search for errors
grep ERROR ./logs/cdk-mintd.log

# Search for Spark events
grep "Spark" ./logs/cdk-mintd.log
```

## Advanced Testing

### Test with CDK CLI

If you have cdk-cli installed:

```bash
# Set mint URL
export MINT_URL=http://127.0.0.1:8085

# Request mint (creates invoice)
cdk-cli mint request 100

# Pay invoice externally, then mint
cdk-cli mint

# Check balance
cdk-cli balance

# Create melt quote
cdk-cli melt quote <bolt11_invoice>

# Melt (pay invoice)
cdk-cli melt
```

### Test Multiple Payments

Create a script to test multiple payments:

```bash
#!/bin/bash
for i in {1..5}; do
  echo "Creating mint quote $i..."
  curl -X POST http://127.0.0.1:8085/v1/mint/quote/bolt11 \
    -H "Content-Type: application/json" \
    -d "{\"amount\": $((100 * i)), \"unit\": \"sat\"}" | jq
  sleep 2
done
```

## Moving to Production

When ready for production:

1. **Generate New Mnemonics**
   ```bash
   # Install bip39 tool
   cargo install bip39-cli
   
   # Generate for CDK mint
   bip39 generate --words 24
   
   # Generate for Spark wallet
   bip39 generate --words 24
   ```

2. **Update Configuration**
   - Change `network = "mainnet"`
   - Use new mnemonics (via environment variables)
   - Get Spark API key from Breez
   - Set proper storage paths
   - Configure backups

3. **Secure the Setup**
   ```bash
   # Lock down config file
   chmod 600 production-config.toml
   
   # Use environment variables
   export CDK_MINT_MNEMONIC="your real mnemonic"
   export SPARK_WALLET_MNEMONIC="your spark mnemonic"
   export SPARK_API_KEY="your api key"
   ```

4. **Use Systemd Service**
   ```ini
   [Unit]
   Description=CDK Mint with Spark
   After=network.target
   
   [Service]
   Type=simple
   User=mint
   WorkingDirectory=/opt/cdk-mint
   ExecStart=/opt/cdk-mint/cdk-mintd --config production-config.toml
   Restart=always
   
   [Install]
   WantedBy=multi-user.target
   ```

## Need Help?

- **Logs**: Check `./logs/cdk-mintd.log` for detailed information
- **Swagger UI**: `http://127.0.0.1:8085/swagger-ui/` for API documentation
- **CDK Docs**: https://github.com/cashubtc/cdk
- **Spark SDK**: https://sdk-doc-spark.breez.technology/
- **Matrix Chat**: #dev:matrix.cashu.space

## Success! 🎉

If you can:
1. Start the mint ✅
2. Create a mint quote ✅
3. Pay an invoice ✅
4. Mint ecash ✅
5. Melt ecash ✅

Then Spark integration is working perfectly!

---

**Happy Testing!** 🧪⚡🥜

