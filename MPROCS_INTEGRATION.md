# mprocs Integration for CDK Regtest

The CDK regtest environment now integrates with `mprocs` to provide a beautiful TUI (Terminal User Interface) for monitoring all component logs in real-time.

## What is mprocs?

`mprocs` is a TUI for running multiple processes and monitoring their output. Perfect for development environments where you need to watch logs from multiple services simultaneously.

## Features

### Automatic Setup
- The regtest script checks for `mprocs` and offers to install it if missing
- Creates a dynamic mprocs configuration with all relevant log files
- Handles missing log files gracefully (waits for them to be created)

### Components Monitored
- **cln-mint**: CDK mint connected to CLN
- **lnd-mint**: CDK mint connected to LND  
- **bitcoind**: Bitcoin regtest node
- **cln-one**: Core Lightning node #1
- **cln-two**: Core Lightning node #2
- **lnd-one**: LND node #1
- **lnd-two**: LND node #2

### Key Benefits
- **Real-time log monitoring** for all components
- **Side-by-side view** of related services
- **Easy navigation** between different logs
- **Scrollback** to review history
- **Search functionality** within logs
- **Process management** (start/stop/restart individual processes)

## Usage

### Automatic (Recommended) - Log Tailing Mode
```bash
just regtest
# After setup completes, mprocs launches automatically
# Mints start and log to files, mprocs shows log contents
```

### Direct Process Management Mode
```bash
just regtest-mprocs
# After setup, mprocs starts with mint processes stopped
# Use 's' key to start individual mints
# Full process control from within mprocs
```

### Manual Launch
```bash
# Start environment without mprocs
just regtest

# In another terminal, launch mprocs
just regtest-logs
```

### Commands Available
```bash
just regtest         # Starts environment and mprocs (log tailing mode)
just regtest-mprocs  # Starts environment with direct process management  
just regtest-logs    # Manual mprocs launch (adapts to current mode)
```

## mprocs Controls

### Direct Process Management Mode:
- **Arrow keys**: Navigate between processes
- **s**: Start the selected process  
- **k**: Kill the selected process
- **r**: Restart the selected process
- **Enter**: Focus on selected process
- **Tab**: Switch between process list and log view
- **?**: Show help
- **q**: Quit mprocs (stops all managed processes)

### Log Tailing Mode:
- **Arrow keys**: Navigate between processes
- **Enter**: Focus on selected process
- **Tab**: Switch between process list and log view
- **PageUp/PageDown**: Scroll through logs
- **Ctrl+C**: Interrupt current process
- **q**: Quit mprocs (processes continue running)

## Installation

If `mprocs` is not installed:
```bash
# Automatic installation prompt when running regtest
just regtest

# Manual installation
cargo install mprocs

# Or via package manager (varies by OS)
# Ubuntu/Debian: apt install mprocs
# macOS: brew install mprocs
```

## Configuration

The mprocs configuration is automatically generated at `$CDK_ITESTS_DIR/mprocs.yaml`. It includes:

- Proper log file paths for all components
- Graceful handling of missing files
- Optimized UI settings for development
- Auto-start for all monitoring processes

## Development Workflow

### Before mprocs:
- Start regtest environment
- Open multiple terminals to `tail -f` different log files
- Manually manage multiple windows/panes
- Switch between terminals to see different components

### With mprocs:
- Start regtest environment → automatic log monitoring
- Single TUI shows all component logs
- Easy navigation between components
- Professional development experience

## Example View

```
┌─Processes─────────┬─Output───────────────────────────────────────┐
│ ● cln-mint        │ 2024-07-08T08:30:12 INFO cdk_mintd: Starting │
│ ● lnd-mint        │ mint server on 127.0.0.1:8085               │
│ ● bitcoind        │ 2024-07-08T08:30:13 INFO: New invoice       │
│ ● cln-one         │ received for 1000 sats                      │
│ ● cln-two         │ 2024-07-08T08:30:14 INFO: Payment           │
│ ● lnd-one         │ successful                                   │
│ ● lnd-two         │                                              │
│                   │                                              │
└───────────────────┴──────────────────────────────────────────────┘
```

## Fallback

If mprocs is not available or fails:
- Environment continues to work normally
- Falls back to simple wait loop
- All `just` commands work as expected
- Logs still accessible via `just regtest-logs`

This integration makes CDK development much more pleasant by providing professional-grade log monitoring out of the box! 🎉
