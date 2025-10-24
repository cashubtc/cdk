# Spark Integration Test Methodology

This document provides a comprehensive testing methodology for the Spark Lightning backend integration with CDK mints.

## Test Environment Setup

### Prerequisites

- Mainnet Spark mint running on https://mint.trailscoffee.com
- Cashu wallet client (cashu.me web app or cdk-cli)
- External Lightning wallet for testing (e.g., Phoenix, Breez, Wallet of Satoshi)
- Test amounts: 10, 21, 50, 100, 500 sats
- Test environment with proper network connectivity
- Access to mint logs and configuration

### Test Configuration Verification

```bash
# Verify mint is running
ps aux | grep cdk-mintd

# Check configuration
grep -A 3 "fee_percent\|reserve_fee_min" mainnet-spark-mint.toml

# Verify HTTPS and DNS
curl -I https://mint.trailscoffee.com/v1/info

# Check keyset
curl -s https://mint.trailscoffee.com/v1/keysets | jq '.keysets[0]'
```

## Flow 1: Receive Lightning → Mint Ecash

### Test Case 1.1: Small Amount (10 sats)

**Objective**: Verify basic Lightning-to-ecash conversion works with minimal amount

**Steps**:
```bash
# Step 1: Create mint quote
QUOTE_RESPONSE=$(curl -s -X POST https://mint.trailscoffee.com/v1/mint/quote/bolt11 \
  -H "Content-Type: application/json" \
  -d '{"amount": 10, "unit": "sat"}')
echo $QUOTE_RESPONSE | jq '.'

# Extract quote ID and invoice
QUOTE_ID=$(echo $QUOTE_RESPONSE | jq -r '.quote')
INVOICE=$(echo $QUOTE_RESPONSE | jq -r '.request')
echo "Quote ID: $QUOTE_ID"
echo "Invoice: $INVOICE"

# Step 2: Pay invoice from external Lightning wallet
# Copy invoice and pay from Phoenix/Breez/WoS

# Step 3: Poll quote status (or check via cashu.me)
curl https://mint.trailscoffee.com/v1/mint/quote/bolt11/$QUOTE_ID | jq '.'

# Step 4: Verify in logs
grep "Transfer claimed" ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d) | tail -1
```

**Expected Results**:
- Quote created successfully with valid invoice
- Invoice can be paid from external wallet
- Quote state changes to "PAID" after payment
- Transfer claimed event appears in logs
- Ecash can be minted successfully

**Success Criteria**:
- Quote creation time < 1 second
- Payment detection time < 5 seconds
- No errors in logs
- Correct amount credited

### Test Case 1.2: Medium Amount (100 sats)

**Objective**: Verify proper fee calculation and larger amount handling

**Steps**: Repeat Test Case 1.1 with 100 sats

**Expected Results**:
- Fee calculation: 100 * 0.005 = 0.5 sats + 1 sat reserve = 1 sat fee
- Total required: 101 sats
- All other criteria from Test Case 1.1

### Test Case 1.3: Large Amount (500 sats)

**Objective**: Test with larger amounts and verify fee scaling

**Steps**: Repeat Test Case 1.1 with 500 sats

**Expected Results**:
- Fee calculation: 500 * 0.005 = 2.5 sats + 1 sat reserve = 3 sats fee
- Total required: 503 sats
- All other criteria from Test Case 1.1

### Test Case 1.4: Edge Cases

**Test 1.4.1: Minimum Amount (1 sat)**
- Test with 1 sat
- Verify minimum fee handling
- Expected: 1 sat + 1 sat reserve = 2 sats total

**Test 1.4.2: Invoice Expiry**
- Create quote and wait for expiry
- Attempt to pay expired invoice
- Expected: Quote expires, payment fails

**Test 1.4.3: Duplicate Payment Attempt**
- Pay same invoice twice
- Expected: Second payment fails gracefully

**Test 1.4.4: Cancelled Invoice**
- Create quote, then cancel
- Expected: Quote becomes invalid

## Flow 2: Send Ecash → Receive Ecash (P2P)

### Test Case 2.1: Basic Token Send

**Objective**: Verify P2P ecash transfer between wallets

**Steps**:
```bash
# Using cashu.me web app or cdk-cli
# Step 1: Generate ecash token from wallet
# Amount: 21 sats

# Step 2: Send token to another device/wallet
# Copy token string

# Step 3: Receive on second device
# Paste token and redeem

# Step 4: Verify both wallets updated correctly
# Sender: -21 sats, Receiver: +21 sats
```

**Expected Results**:
- Token generated successfully
- Token can be sent to another device
- Token can be redeemed on receiving device
- Balances updated correctly on both sides
- No double-spending possible

### Test Case 2.2: Multiple Denomination Token

**Objective**: Test with amounts requiring multiple proof denominations

**Steps**: Repeat Test Case 2.1 with 42 sats

**Expected Results**:
- Correct proof selection for 42 sats
- All denominations valid
- Change handling works correctly

### Test Case 2.3: Partial Redeem

**Objective**: Test partial token redemption

**Steps**:
- Create 100 sat token
- Redeem only 50 sats
- Verify change handling

**Expected Results**:
- 50 sats redeemed successfully
- 50 sats returned as change
- Change token can be used separately

### Test Case 2.4: Edge Cases

**Test 2.4.1: Expired Token**
- Create token, wait for expiry
- Attempt to redeem expired token
- Expected: Redemption fails

**Test 2.4.2: Already-Spent Token**
- Spend token, attempt to spend again
- Expected: Double-spend detection

**Test 2.4.3: Invalid Token Format**
- Attempt to redeem malformed token
- Expected: Validation error

**Test 2.4.4: Cross-Mint Token**
- Attempt to redeem token from different mint
- Expected: Invalid keyset error

## Flow 3: Receive Ecash → Mint Ecash

### Test Case 3.1: Basic Ecash Receive

**Objective**: Verify receiving ecash from external source

**Steps**:
- Receive 42 sat token from external source
- Verify token validates against keyset
- Redeem successfully
- Check wallet balance increases

**Expected Results**:
- Token validates successfully
- Redemption completes without errors
- Balance increases by correct amount
- Token cannot be spent again

### Test Case 3.2: Cross-Mint Receive

**Objective**: Test receiving token from different mint

**Steps**:
- Attempt to redeem token from different mint
- Expected: Error - invalid keyset

**Expected Results**:
- Clear error message about invalid keyset
- No balance change
- Token rejected gracefully

## Flow 4: Melt Ecash → Send Lightning

### Test Case 4.1: Small Lightning Payment (10 sats)

**Objective**: Verify basic ecash-to-Lightning conversion

**Steps**:
```bash
# Step 1: Create external Lightning invoice
# Use Phoenix/Breez to create 10 sat invoice

# Step 2: Get melt quote from mint
MELT_RESPONSE=$(curl -s -X POST https://mint.trailscoffee.com/v1/melt/quote/bolt11 \
  -H "Content-Type: application/json" \
  -d '{"request": "lnbc100n1...", "unit": "sat"}')
echo $MELT_RESPONSE | jq '.'

# Extract quote ID and fee information
MELT_QUOTE_ID=$(echo $MELT_RESPONSE | jq -r '.quote')
FEE_RESERVE=$(echo $MELT_RESPONSE | jq -r '.fee_reserve')
echo "Melt Quote ID: $MELT_QUOTE_ID"
echo "Fee Reserve: $FEE_RESERVE"

# Step 3: Execute melt (via cashu.me or cdk-cli)
# Provide ecash proofs to pay invoice

# Step 4: Verify payment
curl https://mint.trailscoffee.com/v1/melt/quote/bolt11/$MELT_QUOTE_ID | jq '.'

# Step 5: Check logs
grep "Payment completed successfully" ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d) | tail -1

# Step 6: Verify receiver got payment
# Check external wallet received exactly 10 sats
```

**Expected Results**:
- Melt quote created successfully
- Fee calculation is reasonable
- Payment executes successfully
- Receiver gets exact invoice amount
- Sender balance decreases by amount + fees
- Payment appears in logs

### Test Case 4.2: Medium Payment (50 sats)

**Objective**: Test with medium amounts

**Steps**: Repeat Test Case 4.1 with 50 sats

**Expected Results**:
- All criteria from Test Case 4.1
- Fee scales appropriately
- No performance degradation

### Test Case 4.3: With Exact Balance

**Objective**: Test payment with exact available balance

**Steps**:
- Wallet has 110 sats
- Pay 100 sat invoice
- Verify: Sufficient for payment + fees (105 sats needed with 0.5% + 1 sat)

**Expected Results**:
- Payment succeeds
- Correct amount deducted
- No overpayment

### Test Case 4.4: Insufficient Balance

**Objective**: Test error handling with insufficient funds

**Steps**:
- Wallet has 10 sats
- Attempt to pay 20 sat invoice
- Expected: Error - insufficient balance

**Expected Results**:
- Clear error message
- No partial payment
- Balance unchanged

### Test Case 4.5: Edge Cases

**Test 4.5.1: Expired Lightning Invoice**
- Create invoice, wait for expiry
- Attempt to pay expired invoice
- Expected: Payment fails

**Test 4.5.2: Already-Paid Lightning Invoice**
- Pay invoice, attempt to pay again
- Expected: Payment fails

**Test 4.5.3: Invalid Invoice Format**
- Attempt to pay malformed invoice
- Expected: Validation error

**Test 4.5.4: Payment Routing Failure**
- Use unpayable invoice
- Expected: Payment fails with routing error

## Flow 5: Complete Round Trip

### Test Case 5.1: Full Cycle

**Objective**: Test complete payment cycle

**Steps**:
1. Receive 100 sats via Lightning → Mint ecash (balance: 100)
2. Send 42 sats ecash to friend (balance: 58)
3. Receive 21 sats ecash from friend (balance: 79)
4. Pay 50 sat Lightning invoice (balance: ~25-27 after fees)
5. Verify final balance matches expected

**Expected Results**:
- All operations succeed
- Balances are correct at each step
- No funds lost
- All transactions logged

### Test Case 5.2: Multi-User Scenario

**Objective**: Test with multiple users

**Steps**:
- User A: Receives Lightning, sends ecash
- User B: Receives ecash, melts to Lightning
- User C: Receives Lightning from B
- Verify all balances and no funds lost

**Expected Results**:
- All users can complete their operations
- Balances are consistent
- No double-spending
- All transactions properly recorded

## Performance and Stress Tests

### Test Case 6.1: Concurrent Mints

**Objective**: Test concurrent payment processing

**Steps**:
- 5 users simultaneously receive Lightning payments
- Verify all quotes process correctly
- Check for race conditions

**Expected Results**:
- All payments processed successfully
- No race conditions
- Performance remains acceptable
- No data corruption

### Test Case 6.2: Rapid Operations

**Objective**: Test rapid successive operations

**Steps**:
- Quick succession: mint → send → receive → melt
- Verify state consistency

**Expected Results**:
- All operations complete successfully
- State remains consistent
- No errors or corruption

### Test Case 6.3: Large Transaction

**Objective**: Test with large amounts

**Steps**:
- Mint 10,000 sats
- Verify denomination handling
- Melt back to Lightning
- Check fee calculations scale correctly

**Expected Results**:
- Large amounts handled correctly
- Fee calculations scale appropriately
- Performance remains acceptable
- No memory issues

## Regression Tests

### Test Case 7.1: Restart Resilience

**Objective**: Test recovery after restart

**Steps**:
- Create pending melt quote
- Restart mint daemon
- Verify quote state recovers correctly
- Check for "pending melt quotes" log message

**Expected Results**:
- Mint restarts successfully
- Pending quotes are handled correctly
- No data loss
- State is consistent

### Test Case 7.2: Network Interruption

**Objective**: Test network resilience

**Steps**:
- Monitor "Spark stream disconnected" events
- Verify automatic reconnection
- Ensure no payment loss during reconnect

**Expected Results**:
- Automatic reconnection works
- No payment loss during disconnection
- State remains consistent
- Operations resume normally

### Test Case 7.3: Storage Persistence

**Objective**: Test data persistence

**Steps**:
- Perform transactions
- Stop mint
- Check `./data/mint.db` exists
- Restart mint
- Verify historical quotes persist

**Expected Results**:
- Data persists across restarts
- Historical data is accessible
- No data corruption
- State is consistent

## Automated Test Script

### Basic Test Script

```bash
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
```

### Advanced Test Script

```bash
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

# Run tests
echo "Starting Advanced Spark Integration Tests..." | tee $RESULTS_FILE

# Test various amounts
test_payment_flow 10 "1.1"
test_payment_flow 21 "1.2"
test_payment_flow 50 "1.3"
test_payment_flow 100 "1.4"

echo -e "\n[SUMMARY] Advanced tests completed. See $RESULTS_FILE for details."
```

## Success Criteria

### Performance Metrics

1. **Quote Creation Time**: < 1 second
2. **Payment Settlement Time**: < 5 seconds
3. **Success Rate**: > 95% under normal conditions
4. **Error Rate**: < 5% of total operations
5. **Memory Usage**: Stable, no memory leaks
6. **CPU Usage**: Reasonable under load

### Functional Requirements

1. **Correctness**: Exact amounts transferred (accounting for fees)
2. **Reliability**: Consistent behavior across all test cases
3. **Error Handling**: Clear error messages, proper rollback
4. **Logging**: All operations logged with timestamps and status
5. **State Consistency**: Database reflects actual state after all operations
6. **Security**: No unauthorized access or data leakage

### Quality Assurance

1. **Test Coverage**: All payment flows tested
2. **Edge Cases**: All edge cases handled gracefully
3. **Error Scenarios**: All error scenarios tested
4. **Performance**: Performance requirements met
5. **Documentation**: All tests documented and repeatable

## Test Execution Schedule

### Development Phase
- **Smoke Tests** (Flows 1.1, 4.1): After each code change
- **Core Flows** (1-4): Daily during development
- **Edge Cases**: Weekly
- **Performance Tests**: Before major releases

### Production Phase
- **Complete Suite** (All flows): Before each release
- **Stress Tests** (Flow 6): Weekly
- **Regression Tests** (Flow 7): Before production deployment
- **Monitoring**: Continuous monitoring of key metrics

### Maintenance Phase
- **Full Test Suite**: Monthly
- **Performance Review**: Quarterly
- **Security Audit**: Annually
- **Documentation Review**: As needed

## Test Data Management

### Test Wallets
- Use dedicated test wallets for all testing
- Never use production wallets for testing
- Maintain separate test environments
- Clear test data regularly

### Test Amounts
- Use small amounts for initial testing
- Gradually increase amounts for stress testing
- Document all test amounts used
- Track test costs and limits

### Test Logs
- Maintain detailed logs of all tests
- Store logs for analysis and debugging
- Archive old test logs
- Use logs for performance analysis

## Reporting

### Test Reports
- Generate test reports after each test run
- Include success/failure rates
- Document any issues found
- Track performance metrics

### Issue Tracking
- Log all issues found during testing
- Prioritize issues by severity
- Track issue resolution
- Document workarounds

### Performance Monitoring
- Monitor key performance metrics
- Track trends over time
- Alert on performance degradation
- Regular performance reviews

This comprehensive test methodology ensures the Spark integration is thoroughly tested and reliable for production use.
