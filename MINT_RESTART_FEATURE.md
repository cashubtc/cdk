# Mint Restart Feature

A new command has been added to restart the CDK mints after recompiling, perfect for development workflows when you're making changes to the mint code.

## Command

```bash
just restart-mints
```

or

```bash
./misc/regtest_helper.sh restart-mints
```

## What It Does

1. **Stops** both running mints (CLN and LND mints)
2. **Recompiles** the `cdk-mintd` binary with your latest changes
3. **Restarts** both mints with the same configuration
4. **Waits** for both mints to be ready and responding
5. **Updates** the state file with new process IDs

## Development Workflow

### Before this feature:
```bash
# Terminal 1: Start environment
just regtest

# Terminal 2: Make code changes, then manually restart everything
# Ctrl+C in Terminal 1 (stops entire environment including Lightning network)
just regtest  # Start everything again (slow)
```

### With this feature:
```bash
# Terminal 1: Start environment (once)
just regtest

# Terminal 2: Make code changes, then quickly restart just the mints
just restart-mints  # Fast! Keeps Lightning network running
just mint-test      # Test your changes
```

## Benefits

1. **Faster Development Cycle** - No need to restart the entire Lightning network
2. **Preserves Network State** - Bitcoin blockchain, Lightning channels, and node states remain intact
3. **Automatic Recompilation** - No need to manually run `cargo build`
4. **Status Validation** - Ensures mints are responding before completing
5. **State Management** - Updates process IDs for other commands to work correctly

## Example Output

```
===============================
Restarting CDK Mints
===============================
Stopping existing mints...
  Stopping CLN Mint (PID: 12345)
  Stopping LND Mint (PID: 12346)
Recompiling cdk-mintd...
✓ Compilation successful
Starting CLN Mint...
Waiting for CLN mint to start...
✓ CLN Mint ready
Starting LND Mint...
Waiting for LND mint to start...
✓ LND Mint ready

✅ Mints restarted successfully!
  CLN Mint: http://127.0.0.1:8085 (PID: 54321)
  LND Mint: http://127.0.0.1:8087 (PID: 54322)
===============================
```

## Use Cases

- **Testing mint code changes** without restarting the entire regtest environment
- **Debugging mint behavior** with fresh mint instances
- **Performance testing** with clean mint state but preserved Lightning network
- **Integration testing** after mint code modifications

This feature makes the development experience much smoother when working on CDK mint functionality!
