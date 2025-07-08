# mprocs Process Management Modes

The CDK regtest environment now supports two different modes for managing processes with mprocs:

## Mode 1: Log Tailing (Default)
**Command**: `just regtest`

### How it works:
- Mints are started by the bash script and run in the background
- Mints write their output to log files (`mintd.log`)
- mprocs uses `tail -f` to follow these log files
- Log files persist even after mprocs exits

### Pros:
- ✅ Log files are preserved for later analysis
- ✅ Simple setup
- ✅ Works even if mprocs crashes

### Cons:
- ❌ Cannot restart mints from within mprocs
- ❌ Must use external commands to control mints
- ❌ mprocs shows file contents, not live processes

## Mode 2: Direct Process Management
**Command**: `just regtest-mprocs`

### How it works:
- mprocs directly manages the mint processes
- Mints are started/stopped by mprocs itself
- Output goes directly to mprocs (no log files by default)
- Full process control from within mprocs

### Pros:
- ✅ Start/stop/restart mints directly from mprocs
- ✅ Live process output
- ✅ Better development workflow
- ✅ Process status indicators

### Cons:
- ❌ Output not saved to files (unless configured)
- ❌ If mprocs crashes, you lose the processes

## mprocs Controls

### Direct Process Management Mode:
- **Arrow keys**: Navigate between processes
- **s**: Start the selected process
- **k**: Kill the selected process
- **r**: Restart the selected process
- **Enter**: Focus on a process (see its output)
- **Tab**: Switch between process list and output
- **?**: Show help
- **q**: Quit mprocs (stops all managed processes)

### Log Tailing Mode:
- **Arrow keys**: Navigate between log sources
- **Enter**: Focus on a log source
- **Tab**: Switch between process list and log view
- **PageUp/PageDown**: Scroll through logs
- **q**: Quit mprocs (processes continue running)

## Usage Examples

### Start with Log Tailing (Original Mode)
```bash
just regtest
# Mints start automatically and log to files
# mprocs shows log contents
# Use Ctrl+C or 'q' to exit mprocs
# Processes continue running in background
```

### Start with Direct Process Management
```bash
just regtest-mprocs
# Only Lightning network starts automatically
# In mprocs, navigate to "cln-mint" and press 's' to start it
# Navigate to "lnd-mint" and press 's' to start it
# Use 'r' to restart mints after code changes
# Use 'q' to exit and stop all processes
```

### Switch Between Modes

If you started with log tailing mode, you can access the direct management:
```bash
# In another terminal
source /tmp/cdk_regtest_env
just regtest-logs  # This will detect the mode and adapt
```

## Development Workflow Comparison

### Traditional (Log Tailing):
1. `just regtest`
2. Make code changes
3. In another terminal: `just restart-mints`
4. Check logs in mprocs

### Direct Management:
1. `just regtest-mprocs`
2. Press 's' to start mints
3. Make code changes  
4. In mprocs: press 'r' on each mint to restart
5. Watch live output directly

## Technical Details

### Project Root Handling
The direct process management mode ensures that:
- Startup scripts change to the correct project root directory
- Cargo commands run from where the `Cargo.toml` file is located
- Environment variables are properly set before starting processes

### File Structure
```
$CDK_ITESTS_DIR/
├── start_cln_mint.sh       # Sets PROJECT_ROOT and runs cargo from there
├── start_lnd_mint.sh       # Sets PROJECT_ROOT and runs cargo from there
└── mprocs.yaml            # Points to the startup scripts
```

Each startup script:
1. Changes to the project root directory (`cd "$PROJECT_ROOT"`)
2. Sets all required environment variables
3. Executes `cargo run --bin cdk-mintd` from the correct location

The environment variables and helper commands work the same in both modes:
- `just ln-cln1 getinfo`
- `just btc-mine 5`
- `just mint-info`
- etc.
