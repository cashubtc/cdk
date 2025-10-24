#!/bin/bash
# spark-advanced-test.sh

MINT_URL="https://mint.trailscoffee.com"
RESULTS_FILE="advanced-test-results-$(date +%Y%m%d-%H%M%S).log"

# Function to test payment flow
test_payment_flow() {
    local amount=$1
    local test_name=$2
    
    echo -e "\n[TEST $test_name] Testing $amount sat payment flow..." | tee -a $RESULTS_FILE
    
    # Create mint quote
    QUOTE_RESPONSE=$(curl -s -X POST $MINT_URL/v1/mint/quote/bolt11 \
        -H "Content-Type: application/json" \
        -d "{\"amount\": $amount, \"unit\": \"sat\"}")
    
    QUOTE_ID=$(echo $QUOTE_RESPONSE | jq -r '.quote')
    INVOICE=$(echo $QUOTE_RESPONSE | jq -r '.request')
    
    if [ -n "$QUOTE_ID" ] && [ "$QUOTE_ID" != "null" ]; then
        echo "✓ Quote created: $QUOTE_ID" | tee -a $RESULTS_FILE
        
        # Wait for payment (manual step)
        echo "  [MANUAL] Pay invoice: $INVOICE" | tee -a $RESULTS_FILE
        echo "  [MANUAL] Press Enter when payment is complete..." | tee -a $RESULTS_FILE
        read
        
        # Check quote status
        QUOTE_STATUS=$(curl -s $MINT_URL/v1/mint/quote/bolt11/$QUOTE_ID)
        STATE=$(echo $QUOTE_STATUS | jq -r '.state')
        
        if [ "$STATE" = "PAID" ]; then
            echo "✓ Payment confirmed" | tee -a $RESULTS_FILE
        else
            echo "✗ Payment not confirmed (state: $STATE)" | tee -a $RESULTS_FILE
        fi
    else
        echo "✗ Quote creation failed" | tee -a $RESULTS_FILE
    fi
}

# Function to test melt flow
test_melt_flow() {
    local amount=$1
    local test_name=$2
    
    echo -e "\n[TEST $test_name] Testing $amount sat melt flow..." | tee -a $RESULTS_FILE
    
    echo "  [MANUAL] Create Lightning invoice for $amount sats" | tee -a $RESULTS_FILE
    echo "  [MANUAL] Enter Lightning invoice: " | tee -a $RESULTS_FILE
    read INVOICE
    
    if [ -n "$INVOICE" ]; then
        # Create melt quote
        MELT_RESPONSE=$(curl -s -X POST $MINT_URL/v1/melt/quote/bolt11 \
            -H "Content-Type: application/json" \
            -d "{\"request\": \"$INVOICE\", \"unit\": \"sat\"}")
        
        MELT_QUOTE_ID=$(echo $MELT_RESPONSE | jq -r '.quote')
        FEE_RESERVE=$(echo $MELT_RESPONSE | jq -r '.fee_reserve')
        
        if [ -n "$MELT_QUOTE_ID" ] && [ "$MELT_QUOTE_ID" != "null" ]; then
            echo "✓ Melt quote created: $MELT_QUOTE_ID" | tee -a $RESULTS_FILE
            echo "  Fee reserve: $FEE_RESERVE sats" | tee -a $RESULTS_FILE
            echo "  [MANUAL] Execute melt with ecash proofs" | tee -a $RESULTS_FILE
            echo "  [MANUAL] Press Enter when melt is complete..." | tee -a $RESULTS_FILE
            read
            
            # Check melt status
            MELT_STATUS=$(curl -s $MINT_URL/v1/melt/quote/bolt11/$MELT_QUOTE_ID)
            MELT_STATE=$(echo $MELT_STATUS | jq -r '.state')
            
            if [ "$MELT_STATE" = "PAID" ]; then
                echo "✓ Melt confirmed" | tee -a $RESULTS_FILE
            else
                echo "✗ Melt not confirmed (state: $MELT_STATE)" | tee -a $RESULTS_FILE
            fi
        else
            echo "✗ Melt quote creation failed" | tee -a $RESULTS_FILE
        fi
    else
        echo "✗ No invoice provided" | tee -a $RESULTS_FILE
    fi
}

# Function to check logs
check_logs() {
    echo -e "\n[LOG CHECK] Checking recent logs..." | tee -a $RESULTS_FILE
    
    LOG_FILE="$HOME/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d)"
    
    if [ -f "$LOG_FILE" ]; then
        echo "Recent errors:" | tee -a $RESULTS_FILE
        tail -20 "$LOG_FILE" | grep -i error | tail -5 | tee -a $RESULTS_FILE
        
        echo "Recent payment events:" | tee -a $RESULTS_FILE
        tail -20 "$LOG_FILE" | grep -i "transfer claimed\|payment completed" | tail -5 | tee -a $RESULTS_FILE
        
        echo "Spark stream status:" | tee -a $RESULTS_FILE
        tail -20 "$LOG_FILE" | grep -i "spark stream" | tail -3 | tee -a $RESULTS_FILE
    else
        echo "Log file not found: $LOG_FILE" | tee -a $RESULTS_FILE
    fi
}

# Run tests
echo "Starting Advanced Spark Integration Tests..." | tee $RESULTS_FILE

# Basic connectivity tests
echo -e "\n[CONNECTIVITY] Testing basic connectivity..." | tee -a $RESULTS_FILE
if curl -s -I $MINT_URL/v1/info | head -1 | grep -q "200 OK"; then
    echo "✓ Mint is accessible" | tee -a $RESULTS_FILE
else
    echo "✗ Mint is not accessible" | tee -a $RESULTS_FILE
    exit 1
fi

# Test various amounts for mint quotes
test_payment_flow 10 "1.1"
test_payment_flow 21 "1.2"
test_payment_flow 50 "1.3"
test_payment_flow 100 "1.4"

# Test melt flows
test_melt_flow 10 "2.1"
test_melt_flow 50 "2.2"

# Check logs
check_logs

# Performance test
echo -e "\n[PERFORMANCE] Testing quote creation performance..." | tee -a $RESULTS_FILE
for i in {1..5}; do
    start_time=$(date +%s%N)
    QUOTE_RESPONSE=$(curl -s -X POST $MINT_URL/v1/mint/quote/bolt11 \
        -H "Content-Type: application/json" \
        -d '{"amount": 10, "unit": "sat"}')
    end_time=$(date +%s%N)
    
    duration=$(( (end_time - start_time) / 1000000 )) # Convert to milliseconds
    echo "Quote $i: ${duration}ms" | tee -a $RESULTS_FILE
done

echo -e "\n[SUMMARY] Advanced tests completed. See $RESULTS_FILE for details."
