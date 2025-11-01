#!/bin/bash
# spark-integration-test.sh

MINT_URL="https://mint.trailscoffee.com"
TEST_AMOUNTS=(10 21 50 100)
RESULTS_FILE="test-results-$(date +%Y%m%d-%H%M%S).log"

echo "Starting Spark Integration Tests..." | tee $RESULTS_FILE

# Test 1: Mint Info
echo -e "\n[TEST 1] Checking mint info..." | tee -a $RESULTS_FILE
MINT_INFO=$(curl -s $MINT_URL/v1/info)
echo $MINT_INFO | jq '.' >> $RESULTS_FILE
if [ $? -eq 0 ]; then
    echo "✓ Mint info accessible" | tee -a $RESULTS_FILE
else
    echo "✗ Mint info failed" | tee -a $RESULTS_FILE
fi

# Test 2: Keysets
echo -e "\n[TEST 2] Checking keysets..." | tee -a $RESULTS_FILE
KEYSET_RESPONSE=$(curl -s $MINT_URL/v1/keysets)
KEYSET_ID=$(echo $KEYSET_RESPONSE | jq -r '.keysets[0].id')
if [ -n "$KEYSET_ID" ] && [ "$KEYSET_ID" != "null" ]; then
    echo "✓ Keyset ID: $KEYSET_ID" | tee -a $RESULTS_FILE
else
    echo "✗ No keyset found" | tee -a $RESULTS_FILE
fi

# Test 3: Mint quotes for various amounts
for amount in "${TEST_AMOUNTS[@]}"; do
    echo -e "\n[TEST 3.$amount] Creating mint quote for $amount sats..." | tee -a $RESULTS_FILE
    QUOTE_RESPONSE=$(curl -s -X POST $MINT_URL/v1/mint/quote/bolt11 \
        -H "Content-Type: application/json" \
        -d "{\"amount\": $amount, \"unit\": \"sat\"}")
    QUOTE_ID=$(echo $QUOTE_RESPONSE | jq -r '.quote')
    INVOICE=$(echo $QUOTE_RESPONSE | jq -r '.request')
    
    if [ -n "$QUOTE_ID" ] && [ "$QUOTE_ID" != "null" ]; then
        echo "✓ Quote created: $QUOTE_ID" | tee -a $RESULTS_FILE
        echo "  Invoice: ${INVOICE:0:50}..." | tee -a $RESULTS_FILE
        echo "  [MANUAL] Pay this invoice and verify settlement" | tee -a $RESULTS_FILE
    else
        echo "✗ Quote creation failed" | tee -a $RESULTS_FILE
    fi
done

# Test 4: Melt quote creation
echo -e "\n[TEST 4] Testing melt quote creation..." | tee -a $RESULTS_FILE
# Note: This requires a valid Lightning invoice
echo "  [MANUAL] Create Lightning invoice and test melt quote" | tee -a $RESULTS_FILE

echo -e "\n[SUMMARY] Tests completed. See $RESULTS_FILE for details."
