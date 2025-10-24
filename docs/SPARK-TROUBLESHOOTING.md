# Spark Integration Troubleshooting Guide

This guide provides comprehensive troubleshooting information for the Spark Lightning backend integration with CDK mints.

## Known Issues and Solutions

### 1. Session Not Found Errors

**Symptom**: 
```
ERROR spark::operator::rpc::spark_rpc_client: Failed to get operator session from session manager: Generic error: Session not found
```

**Root Cause**: Invalid or expired Breez API key, or Spark service initialization issue

**Solutions**:
- Comment out `api_key` in config to use default Spark service
- Request new API key from Breez
- Verify `storage_dir` is configured and writable
- Check Spark wallet has been initialized with funds

**Prevention**:
- Keep API keys secure and rotate regularly
- Monitor API key expiration dates
- Use environment variables for sensitive configuration

### 2. Transfer Locked Errors

**Symptom**: 
```
leaf is not available to transfer, status: TRANSFER_LOCKED
```

**Root Cause**: Spark wallet leaves are locked from previous failed transactions

**Solutions**:
- Wait 10-15 minutes for locks to expire
- Restart mint daemon to refresh Spark wallet state
- Check Spark wallet balance has sufficient unlocked funds
- Verify no pending transactions in Spark wallet

**Prevention**:
- Avoid rapid successive payment attempts
- Monitor wallet balance before large transactions
- Implement proper error handling and retry logic

### 3. Service Provider Errors

**Symptom**: 
```
Tree service error: Service error: service provider error: graphql error: Something went wrong
```

**Root Cause**: Breez service provider connectivity issues

**Solutions**:
- Check network connectivity
- Verify Breez service status
- Wait and retry after 5-10 minutes
- Check if API key has rate limits

**Prevention**:
- Implement exponential backoff for retries
- Monitor service provider status
- Use multiple service providers if available

### 4. Leaf Verification Warnings

**Symptom**: 
```
WARN spark::tree::service: Leaf's verifying public key does not match the expected value
```

**Root Cause**: Spark wallet state mismatch after restart

**Impact**: Warnings only, not blocking functionality

**Solutions**:
- These are informational warnings from Spark SDK
- Monitor for actual payment failures
- If payments fail, clear storage_dir and reinitialize

**Prevention**:
- Ensure clean shutdown of mint daemon
- Backup wallet state before major updates
- Use consistent storage directory paths

### 5. Payment Amount Discrepancy

**Symptom**: Wallet shows different amount deducted than invoice amount

**Root Cause**: Fixed in current implementation (invoice amount calculation)

**Verification**: Check logs for "Payment completed successfully" with correct amount

**Prevention**:
- Always use invoice amount for payment calculations
- Implement proper amount validation
- Log all payment amounts for audit trails

### 6. Duplicate Payment ID Errors

**Symptom**: 
```
ERROR cdk_sql_common::mint: Payment ID already exists: 019a183b-d67c-7dc6-a963-1a166757d832
```

**Root Cause**: Attempting to process the same payment twice

**Solutions**:
- Check if payment was already processed
- Clear duplicate entries from database
- Implement idempotency checks

**Prevention**:
- Use unique payment identifiers
- Implement proper duplicate detection
- Clear old pending quotes regularly

## Diagnostic Commands

### Basic Health Checks

```bash
# Check mint status
curl -s https://mint.trailscoffee.com/v1/info | jq '.'

# Check keysets
curl -s https://mint.trailscoffee.com/v1/keysets | jq '.'

# Test mint quote creation
curl -s -X POST https://mint.trailscoffee.com/v1/mint/quote/bolt11 \
  -H "Content-Type: application/json" \
  -d '{"amount": 10, "unit": "sat"}' | jq '.'
```

### Log Analysis

```bash
# Check logs for errors
tail -100 ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d) | grep -i error

# Check Spark stream connectivity
grep "Spark stream" ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d) | tail -5

# Monitor payment events
grep "Transfer claimed\|Payment completed" ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d) | tail -10

# Check for session errors
grep -i "session not found\|service provider\|graphql error" ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d) | tail -5

# Monitor transfer locked errors
grep -i "transfer_locked\|leaf.*not available" ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d) | tail -5
```

### System Status

```bash
# Check if mint is running
ps aux | grep cdk-mintd

# Check disk space
df -h

# Check memory usage
free -h

# Check network connectivity
ping -c 3 api.breez.technology
```

### Configuration Verification

```bash
# Check configuration file
cat mainnet-spark-mint.toml | grep -E "(api_key|storage_dir|fee_percent|reserve_fee_min)"

# Verify storage directory exists and is writable
ls -la ./data/spark-mainnet/
touch ./data/spark-mainnet/test-write && rm ./data/spark-mainnet/test-write

# Check environment variables
env | grep CDK_MINTD_SPARK
```

## Log Analysis Patterns

### Successful Incoming Payment

```
INFO spark::signer::default_signer: signature: Signature { ... }
INFO cdk_spark: Transfer claimed event: TransferId(...)
INFO cdk::mint: Mint quote ... amount paid was 0 is now 100
```

### Successful Outgoing Payment

```
INFO cdk_spark: Paying Lightning invoice: lnbc...
INFO post_melt_bolt11:melt:make_payment: cdk_spark: Payment completed successfully
```

### Failed Payment Indicators

```
ERROR post_melt_bolt11:melt: Error returned attempting to pay: ... Spark wallet error
INFO post_melt_bolt11:melt: cdk::mint::melt: Lightning payment for quote ... failed
INFO cdk::mint::proof_writer: Rollback N proofs to their original states
```

### Connection Issues

```
ERROR spark::events::server_stream: Error receiving event, reconnecting: status: Internal
WARN cdk_spark: Spark stream disconnected
INFO cdk_spark: Spark stream connected
```

## Common Error Scenarios

### Scenario 1: Mint Won't Start

**Symptoms**:
- Mint daemon fails to start
- Configuration parsing errors
- Missing dependencies

**Diagnosis**:
```bash
# Check configuration syntax
./target/release/cdk-mintd --config mainnet-spark-mint.toml --check-config

# Check for missing features
cargo build --features spark --package cdk-mintd

# Check logs
tail -50 ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d)
```

**Solutions**:
- Fix configuration syntax errors
- Enable required features
- Check file permissions
- Verify all dependencies are installed

### Scenario 2: Payments Not Detected

**Symptoms**:
- Lightning payments sent but not credited
- Quotes remain in "PENDING" state
- No transfer claimed events in logs

**Diagnosis**:
```bash
# Check if Spark stream is connected
grep "Spark stream connected" ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d) | tail -1

# Check for transfer events
grep "Transfer claimed" ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d) | tail -5

# Check payment cache
grep "incoming_payments" ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d) | tail -5
```

**Solutions**:
- Restart mint daemon
- Check Spark wallet balance
- Verify invoice was paid correctly
- Clear and rebuild payment cache

### Scenario 3: Outgoing Payments Fail

**Symptoms**:
- Melt quotes created but payments fail
- "TRANSFER_LOCKED" errors
- Payment rollbacks

**Diagnosis**:
```bash
# Check for transfer locked errors
grep -i "transfer_locked" ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d) | tail -5

# Check Spark wallet balance
grep "Spark wallet" ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d) | tail -5

# Check payment attempts
grep "make_payment" ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d) | tail -5
```

**Solutions**:
- Wait for locks to expire (10-15 minutes)
- Check Spark wallet has sufficient unlocked funds
- Verify invoice is valid and not expired
- Restart mint daemon to refresh state

### Scenario 4: High Error Rates

**Symptoms**:
- Frequent "Session not found" errors
- Multiple connection drops
- Inconsistent payment success

**Diagnosis**:
```bash
# Count error types
grep -c "Session not found" ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d)
grep -c "Spark stream disconnected" ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d)
grep -c "Payment completed successfully" ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d)

# Check error patterns
grep "ERROR" ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d) | tail -20
```

**Solutions**:
- Check API key validity
- Verify network stability
- Monitor service provider status
- Consider using different service provider
- Implement better retry logic

## Performance Monitoring

### Key Metrics to Monitor

1. **Payment Success Rate**: >95% under normal conditions
2. **Quote Creation Time**: <1 second
3. **Payment Settlement Time**: <5 seconds
4. **Connection Stability**: Minimal disconnections
5. **Error Rate**: <5% of total operations

### Monitoring Commands

```bash
# Calculate success rate
TOTAL_PAYMENTS=$(grep -c "make_payment" ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d))
SUCCESSFUL_PAYMENTS=$(grep -c "Payment completed successfully" ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d))
SUCCESS_RATE=$((SUCCESSFUL_PAYMENTS * 100 / TOTAL_PAYMENTS))
echo "Payment success rate: $SUCCESS_RATE%"

# Monitor connection stability
grep "Spark stream" ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d) | tail -10

# Check error frequency
grep "ERROR" ~/.cdk-mintd/logs/cdk-mintd.log.$(date +%Y-%m-%d) | wc -l
```

## Emergency Procedures

### Complete Reset

If the mint becomes completely unresponsive:

1. **Stop the mint daemon**:
   ```bash
   pkill -f cdk-mintd
   ```

2. **Backup current state**:
   ```bash
   cp -r ./data ./data-backup-$(date +%Y%m%d-%H%M%S)
   ```

3. **Clear problematic state**:
   ```bash
   rm -rf ./data/spark-mainnet/*
   rm -f ./data/mint.db
   ```

4. **Restart with clean state**:
   ```bash
   ./target/release/cdk-mintd --config mainnet-spark-mint.toml
   ```

### Partial Recovery

If only some functions are affected:

1. **Restart mint daemon**:
   ```bash
   pkill -f cdk-mintd && sleep 5 && ./target/release/cdk-mintd --config mainnet-spark-mint.toml
   ```

2. **Clear pending quotes**:
   ```bash
   # This will be handled automatically on restart
   ```

3. **Verify functionality**:
   ```bash
   curl -s https://mint.trailscoffee.com/v1/info | jq '.'
   ```

## Prevention Best Practices

1. **Regular Monitoring**: Set up automated monitoring for key metrics
2. **Backup Strategy**: Regular backups of wallet state and configuration
3. **Update Management**: Keep dependencies and API keys current
4. **Error Handling**: Implement proper retry logic and error recovery
5. **Documentation**: Keep detailed logs of all changes and issues
6. **Testing**: Regular testing of all payment flows
7. **Security**: Secure storage of sensitive configuration data

## Getting Help

If issues persist after following this guide:

1. **Collect Information**:
   - Recent logs (last 100 lines)
   - Configuration file (sanitized)
   - Error messages and timestamps
   - System information

2. **Check Known Issues**:
   - Review this troubleshooting guide
   - Check GitHub issues
   - Search community forums

3. **Escalate**:
   - Contact Breez support for API issues
   - Open GitHub issue for CDK-specific problems
   - Seek community help for configuration issues

## Version Information

- **CDK Version**: Latest with Spark integration
- **Spark SDK Version**: As specified in Cargo.toml
- **Rust Version**: As specified in rust-toolchain.toml
- **Last Updated**: $(date)
