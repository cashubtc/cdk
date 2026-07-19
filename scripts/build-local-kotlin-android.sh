#!/usr/bin/env bash
# Build local cdk-jvm + cdk-android from this CDK tree and publish to mavenLocal.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION="${CDK_LOCAL_VERSION:-0.17.3-rc.0-local-melt-fix}"
NDK_HOME="${ANDROID_NDK_HOME:-/opt/homebrew/share/android-ndk}"
PREBUILT="$(ls -d "$NDK_HOME/toolchains/llvm/prebuilt/"* 2>/dev/null | head -1)"
if [[ -z "${PREBUILT:-}" || ! -d "$PREBUILT/bin" ]]; then
  echo "ANDROID_NDK_HOME invalid: $NDK_HOME" >&2
  exit 1
fi
BIN="$PREBUILT/bin"
API_LEVEL="${ANDROID_API_LEVEL:-24}"

echo "==> Using NDK: $NDK_HOME"
echo "==> Publishing version: $VERSION"
echo "==> CDK: $(git rev-parse --short HEAD) ($(git branch --show-current))"

echo "==> Building host cdk-ffi-kotlin (release)"
cargo build --release -p cdk-ffi-kotlin
HOST_LIB="target/release/libcdk_ffi.dylib"
if [[ ! -f "$HOST_LIB" ]]; then
  HOST_LIB="$(find target -path '*/release/libcdk_ffi.dylib' | head -1)"
fi
test -f "$HOST_LIB"

echo "==> Generating Kotlin UniFFI sources from $HOST_LIB"
mkdir -p target/bindings/kotlin
cargo run --release -p cdk-ffi-kotlin --bin uniffi-bindgen -- generate \
  --library "$HOST_LIB" \
  --language kotlin \
  --out-dir target/bindings/kotlin \
  --no-format

echo "==> Copying sources into bindings/kotlin/cdk-jvm"
rm -rf bindings/kotlin/cdk-jvm/src/main/kotlin/org
mkdir -p bindings/kotlin/cdk-jvm/src/main/kotlin
mkdir -p bindings/kotlin/cdk-jvm/src/main/resources
cp -R target/bindings/kotlin/org bindings/kotlin/cdk-jvm/src/main/kotlin/
cp "$HOST_LIB" bindings/kotlin/cdk-jvm/src/main/resources/libcdk_ffi.dylib

build_android() {
  local rust_target="$1"
  local abi="$2"
  local clang="$3"
  local cc_key="${rust_target//-/_}"
  local cargo_key
  cargo_key="$(echo "$rust_target" | tr 'a-z-' 'A-Z_')"

  echo "==> Building $rust_target ($abi)"
  export "CC_${cc_key}=$BIN/$clang"
  export "AR_${cc_key}=$BIN/llvm-ar"
  export "CARGO_TARGET_${cargo_key}_LINKER=$BIN/$clang"
  export "CARGO_TARGET_${cargo_key}_RUSTFLAGS=-C link-arg=-Wl,-z,max-page-size=16384 -C link-arg=-Wl,-z,common-page-size=16384"

  cargo build --release -p cdk-ffi-kotlin --target "$rust_target"

  local out="bindings/kotlin/cdk-android/src/main/jniLibs/$abi"
  mkdir -p "$out"
  cp "target/$rust_target/release/libcdk_ffi.so" "$out/libcdk_ffi.so"
  echo "    → $out/libcdk_ffi.so"
}

build_android "aarch64-linux-android" "arm64-v8a" "aarch64-linux-android${API_LEVEL}-clang"
build_android "x86_64-linux-android" "x86_64" "x86_64-linux-android${API_LEVEL}-clang"

echo "==> Setting VERSION_NAME=$VERSION"
PROP=bindings/kotlin/gradle.properties
if grep -q '^VERSION_NAME=' "$PROP"; then
  sed -i.bak "s/^VERSION_NAME=.*/VERSION_NAME=$VERSION/" "$PROP"
  rm -f "$PROP.bak"
else
  echo "VERSION_NAME=$VERSION" >> "$PROP"
fi

printf 'sdk.dir=%s\n' "${ANDROID_HOME:-/Users/asm/Library/Android/sdk}" > bindings/kotlin/local.properties

echo "==> Publishing to mavenLocal"
cd bindings/kotlin
./gradlew --no-daemon \
  :cdk-jvm:publishMavenPublicationToMavenLocal \
  :cdk-android:publishReleasePublicationToMavenLocal \
  -x signMavenPublication \
  -x signReleasePublication || \
./gradlew --no-daemon \
  :cdk-jvm:publishMavenPublicationToMavenLocal \
  :cdk-android:publishReleasePublicationToMavenLocal

echo ""
echo "✅ Published org.cashudevkit:cdk-android:$VERSION to ~/.m2"
