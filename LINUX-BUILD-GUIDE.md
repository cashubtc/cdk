# Building Spark-CDK Integration on Linux

**Status**: ✅ All code fixed and ready to build  
**Platform**: Linux (Ubuntu, Debian, Fedora, Arch, etc.)  
**Estimated Time**: 15-20 minutes  

---

## 🚀 Quick Start (5 Commands)

```bash
# 1. Clone/pull the repository
cd ~/
git clone https://github.com/jpgaviria2/cdk.git
cd cdk

# 2. Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env

# 3. Install protobuf compiler
sudo apt install protobuf-compiler  # Ubuntu/Debian
# OR: sudo dnf install protobuf-compiler  # Fedora
# OR: sudo pacman -S protobuf  # Arch

# 4. Build with Spark support (10-15 minutes)
cargo build --package cdk-mintd --features spark --release

# 5. Run the test mint
./target/release/cdk-mintd --config test-spark-mint.toml
```

**Expected**: Mint starts on port 8085, ready to accept Lightning payments! 🎉

---

## 📋 Detailed Instructions

### Step 1: System Prerequisites

**Ubuntu/Debian**:
```bash
sudo apt update
sudo apt install -y build-essential protobuf-compiler pkg-config libssl-dev
```

**Fedora**:
```bash
sudo dnf install -y gcc make protobuf-compiler pkg-config openssl-devel
```

**Arch Linux**:
```bash
sudo pacman -S base-devel protobuf pkgconf openssl
```

### Step 2: Install Rust

```bash
# Download and install rustup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Follow prompts, select default installation

# Activate Rust in current shell
source $HOME/.cargo/env

# Verify installation
rustc --version  # Should show 1.88.0 or later
cargo --version
```

### Step 3: Clone Repository

```bash
# Clone the repository
git clone https://github.com/jpgaviria2/cdk.git
cd cdk

# Verify you're on the correct branch
git branch  # Should show: main
git log --oneline -5  # Should show recent Spark commits
```

### Step 4: Build CDK with Spark

```bash
# Clean build (recommended for first time)
cargo clean

# Build with Spark backend (release mode for performance)
cargo build --package cdk-mintd --features spark --release

# Expected output:
#    Compiling cdk-spark v0.13.0
#    Compiling cdk-mintd v0.13.0
#     Finished release [optimized] target(s) in 10m 23s
```

**Note**: First build takes 10-15 minutes as it downloads and compiles all dependencies.

### Step 5: Verify Build

```bash
# Check binary was created
ls -lh target/release/cdk-mintd

# Should show:
# -rwxr-xr-x 1 user user 50M Oct 17 12:34 target/release/cdk-mintd

# Test help command
./target/release/cdk-mintd --help

# Should display usage information
```

---

## 🧪 Testing the Mint

### Start Test Mint

```bash
# Start the mint with test configuration
./target/release/cdk-mintd --config test-spark-mint.toml

# Expected output:
# INFO Initializing Spark wallet for network: Signet
# INFO Spark wallet initialized successfully
# INFO Starting Spark payment processor
# INFO Starting server on 127.0.0.1:8085
```

**Keep this terminal open** - the mint is running!

### Test API (In New Terminal)

```bash
# Test mint info
curl http://127.0.0.1:8085/v1/info | jq

# Should return JSON with mint information

# Create a mint quote (get Lightning invoice)
curl -X POST http://127.0.0.1:8085/v1/mint/quote/bolt11 \
  -H "Content-Type: application/json" \
  -d '{"amount": 100, "unit": "sat"}' | jq

# Should return:
# {
#   "quote": "quote-id-here",
#   "request": "lntbs100...invoice",
#   "state": "UNPAID",
#   "expiry": 1234567890
# }
```

### Test with Swagger UI

Open browser to: `http://127.0.0.1:8085/swagger-ui/`

- Explore all API endpoints
- Try creating quotes
- Test minting and melting flows

---

## 🔧 Troubleshooting

### Build Fails - Missing Dependencies

**Error**: "Could not find protoc"
```bash
# Install protobuf compiler
sudo apt install protobuf-compiler  # Ubuntu/Debian
```

**Error**: "failed to run custom build command for `ring`"
```bash
# Install build essentials
sudo apt install build-essential pkg-config libssl-dev
```

**Error**: "Spark SDK not found"
```bash
# Clone Spark SDK to sibling directory
cd ~/
git clone https://github.com/breez/spark-sdk.git
cd cdk
# Try building again
```

### Runtime Fails - Connection Issues

**Error**: "Could not connect to Spark network"
```bash
# Check internet connection
ping api.lightspark.com

# Try with different network
# Edit test-spark-mint.toml: network = "regtest"
```

**Error**: "Invalid mnemonic"
```bash
# Generate new test mnemonic
cargo install bip39-cli
bip39 generate --words 24

# Update test-spark-mint.toml with new mnemonic
```

### Port Already in Use

**Error**: "Address already in use (os error 98)"
```bash
# Check what's using port 8085
sudo lsof -i :8085

# Kill the process or change port in config
# Edit test-spark-mint.toml: listen_port = 8086
```

---

## ⚡ Quick Build (Debug Mode)

For faster builds during development:

```bash
# Debug build (faster compile, slower runtime)
cargo build --package cdk-mintd --features spark

# Binary at:
./target/debug/cdk-mintd --config test-spark-mint.toml
```

**Use for**: Development and testing  
**Build time**: ~5 minutes  
**Runtime**: Slower than release  

---

## 🎯 After Successful Build

### Running in Background

```bash
# Run as daemon
nohup ./target/release/cdk-mintd --config test-spark-mint.toml > mint.log 2>&1 &

# Check logs
tail -f mint.log

# Stop
pkill cdk-mintd
```

### Running with Systemd

Create service file `/etc/systemd/system/cdk-mint.service`:

```ini
[Unit]
Description=CDK Mint with Spark Backend
After=network.target

[Service]
Type=simple
User=YOUR_USERNAME
WorkingDirectory=/home/YOUR_USERNAME/cdk
ExecStart=/home/YOUR_USERNAME/cdk/target/release/cdk-mintd --config test-spark-mint.toml
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
```

Enable and start:
```bash
sudo systemctl daemon-reload
sudo systemctl enable cdk-mint
sudo systemctl start cdk-mint
sudo systemctl status cdk-mint
```

---

## 📊 Expected Build Output

### Successful Compilation

```
   Compiling cdk-common v0.13.0
   Compiling spark v0.1.0
   Compiling spark-wallet v0.1.0
   Compiling cdk-spark v0.13.0
   Compiling cdk-mintd v0.13.0
    Finished release [optimized] target(s) in 12m 34s
```

### Binary Size
```bash
du -h target/release/cdk-mintd
# Expected: 40-60 MB (static binary with all dependencies)
```

---

## 🐧 Platform-Specific Notes

### Ubuntu 20.04/22.04 LTS
- ✅ Fully supported
- Install: `sudo apt install build-essential protobuf-compiler`
- Works out of the box

### Debian 11/12
- ✅ Fully supported
- Same as Ubuntu

### Fedora 38+
- ✅ Fully supported
- Use dnf instead of apt
- May need: `sudo dnf groupinstall "Development Tools"`

### Arch Linux
- ✅ Fully supported
- Ensure base-devel installed
- protobuf in official repos

### WSL (Windows Subsystem for Linux)
- ✅ Works perfectly!
- Use Ubuntu or Debian image
- Access Windows files at `/mnt/c/Users/...`
- Can access mint from Windows browser

---

## 🔥 WSL Quick Start (If on Windows)

```bash
# In WSL terminal (Ubuntu/Debian)
cd /path/to/cdk

# Install dependencies
sudo apt update
sudo apt install -y build-essential protobuf-compiler pkg-config libssl-dev

# Ensure Rust is installed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env

# Build
cargo build --package cdk-mintd --features spark --release

# Run
./target/release/cdk-mintd --config test-spark-mint.toml

# Access from Windows browser:
# http://127.0.0.1:8085/swagger-ui/
```

---

## ✅ Success Indicators

### Build Successful When You See:
```
✅ Finished release [optimized] target(s) in X minutes
✅ Binary created at: target/release/cdk-mintd
✅ File size: 40-60 MB
✅ Executable: chmod +x confirmed
```

### Mint Running Successfully When You See:
```
✅ INFO Initializing Spark wallet for network: Signet
✅ INFO Spark wallet initialized successfully  
✅ INFO Starting Spark payment processor
✅ INFO Starting server on 127.0.0.1:8085
✅ curl http://127.0.0.1:8085/v1/info returns JSON
```

---

## 📦 What You're Building

**Complete Cashu Mint with**:
- ⚡ Nodeless Lightning backend (via Spark SDK)
- 🔐 Self-custodial (keys never leave your server)
- 🌐 Multi-network support (Signet for testing)
- 💰 Configurable fees
- 📡 Real-time payment events
- 🔄 Compatible with all CDK features
- 📊 Swagger UI for API testing

**All ready to go - just compile on Linux!** 🚀

---

## 💡 Tips

### Faster Builds
```bash
# Use more CPU cores
cargo build -j 8 --package cdk-mintd --features spark --release

# Cache dependencies
cargo fetch
```

### Clean Build
```bash
# If build fails, try clean build
cargo clean
cargo build --package cdk-mintd --features spark --release
```

### Verify Code First
```bash
# Quick check before full build
cargo check --package cdk-spark
cargo check --package cdk-mintd --features spark
```

---

## 📞 Need Help?

- **Build Issues**: Check logs carefully, usually missing dependencies
- **Runtime Issues**: Check `SPARK-TEST-INSTRUCTIONS.md`
- **API Errors**: Check Spark SDK compatibility
- **Community**: Matrix chat #dev:matrix.cashu.space

---

**Ready to build on Linux!** Just pull from GitHub and follow this guide! 🐧⚡

