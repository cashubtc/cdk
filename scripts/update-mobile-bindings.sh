#!/usr/bin/env bash

set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CDK_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OVERLAY_DIR="$SCRIPT_DIR/mobile-bindings"
DEFAULT_WALLET_ROOT="$(cd "$CDK_ROOT/.." && pwd)/cashu-wallet"

WALLET_ROOT="${CASHU_WALLET_ROOT:-$DEFAULT_WALLET_ROOT}"
BUILD_IOS=1
BUILD_ANDROID=1
VERIFY_WALLETS=1
KEEP_BUILD_DIR="${KEEP_BUILD_DIR:-0}"
ANDROID_API="${ANDROID_API:-26}"
IOS_DEPLOYMENT_TARGET="${IOS_DEPLOYMENT_TARGET:-18.0}"

usage() {
    sed -n '20,48p' "$0" | sed -n 's/^# //p'
}

# Build the current CDK working tree's Swift and Kotlin bindings and install
# them as local packages in the Cashu wallet repository.
#
# Usage:
#   scripts/update-mobile-bindings.sh [options]
#
# Options:
#   --wallet-root PATH       Cashu wallet checkout (default: sibling cashu-wallet)
#   --ios-only               Build and integrate only Swift/iOS
#   --android-only           Build and integrate only Kotlin/Android
#   --skip-wallet-build      Do not compile the wallet apps after integration
#   --keep-build-dir         Preserve the temporary build directory
#   -h, --help               Show this help
#
# Environment overrides:
#   CASHU_WALLET_ROOT, CARGO, RUSTC, RUSTDOC, DEVELOPER_DIR,
#   ANDROID_NDK_HOME, ANDROID_API, IOS_DEPLOYMENT_TARGET, KEEP_BUILD_DIR
#
# The script copies the current working tree, including uncommitted changes,
# into a temporary directory. The federation FFI adapter is applied only to
# that copy; no files under crates/ or bindings/ in the live CDK tree change.

log() {
    printf '\n==> %s\n' "$*"
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

while (($# > 0)); do
    case "$1" in
        --wallet-root)
            (($# >= 2)) || die "--wallet-root requires a path"
            WALLET_ROOT="$2"
            shift 2
            ;;
        --ios-only)
            BUILD_IOS=1
            BUILD_ANDROID=0
            shift
            ;;
        --android-only)
            BUILD_IOS=0
            BUILD_ANDROID=1
            shift
            ;;
        --skip-wallet-build)
            VERIFY_WALLETS=0
            shift
            ;;
        --keep-build-dir)
            KEEP_BUILD_DIR=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown option: $1"
            ;;
    esac
done

[[ -d "$WALLET_ROOT/ios" ]] || die "iOS wallet directory not found under $WALLET_ROOT"
[[ -d "$WALLET_ROOT/android" ]] || die "Android wallet directory not found under $WALLET_ROOT"
[[ -f "$OVERLAY_DIR/federation-ffi.patch" ]] || die "federation FFI patch is missing"
[[ -f "$OVERLAY_DIR/federation.rs" ]] || die "federation FFI module is missing"

require_command patch
require_command perl
require_command rsync

toolchain_channel="$(sed -nE 's/^[[:space:]]*channel[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/p' "$CDK_ROOT/rust-toolchain.toml")"
[[ -n "$toolchain_channel" ]] || die "could not read the Rust toolchain channel"

if [[ -n "${CARGO:-}" ]]; then
    CARGO_BIN="$CARGO"
else
    CARGO_BIN=""
    while IFS= read -r candidate; do
        if [[ "$candidate" == *"/$toolchain_channel-"*/bin/cargo ]]; then
            CARGO_BIN="$candidate"
            break
        fi
    done < <(find "$HOME/.rustup/toolchains" -path '*/bin/cargo' -type f 2>/dev/null | sort)
    if [[ -z "$CARGO_BIN" ]] && command -v cargo >/dev/null 2>&1; then
        CARGO_BIN="$(command -v cargo)"
    fi
fi
[[ -x "$CARGO_BIN" ]] || die "cargo for Rust $toolchain_channel was not found"

TOOLCHAIN_BIN="$(dirname "$CARGO_BIN")"
RUSTC_BIN="${RUSTC:-$TOOLCHAIN_BIN/rustc}"
RUSTDOC_BIN="${RUSTDOC:-$TOOLCHAIN_BIN/rustdoc}"
[[ -x "$RUSTC_BIN" ]] || die "rustc was not found next to cargo"
[[ -x "$RUSTDOC_BIN" ]] || die "rustdoc was not found next to cargo"
export RUSTC="$RUSTC_BIN"
export RUSTDOC="$RUSTDOC_BIN"
export PATH="$TOOLCHAIN_BIN:$PATH"

log "Using Rust $toolchain_channel from $TOOLCHAIN_BIN"

BUILD_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/cdk-mobile-bindings.XXXXXX")"
SOURCE_ROOT="$BUILD_ROOT/cdk"

cleanup() {
    local exit_code=$?
    if [[ "$KEEP_BUILD_DIR" == "1" ]]; then
        printf '\nBuild directory preserved at %s\n' "$BUILD_ROOT"
    else
        rm -rf "$BUILD_ROOT"
    fi
    return "$exit_code"
}
trap cleanup EXIT

log "Copying the current CDK working tree"
mkdir -p "$SOURCE_ROOT"
rsync -a \
    --exclude '.git/' \
    --exclude 'target/' \
    "$CDK_ROOT/" "$SOURCE_ROOT/"

log "Applying the federation FFI adapter to the temporary source copy"
if ! patch --dry-run --silent -d "$SOURCE_ROOT" -p1 -i "$OVERLAY_DIR/federation-ffi.patch"; then
    die "the federation FFI adapter no longer applies; update scripts/mobile-bindings for the latest CDK API"
fi
patch --silent -d "$SOURCE_ROOT" -p1 -i "$OVERLAY_DIR/federation-ffi.patch"
cp "$OVERLAY_DIR/federation.rs" "$SOURCE_ROOT/crates/cdk-ffi/src/federation.rs"

for target in \
    aarch64-apple-ios \
    aarch64-apple-ios-sim \
    x86_64-apple-ios \
    aarch64-linux-android \
    armv7-linux-androideabi \
    i686-linux-android \
    x86_64-linux-android; do
    if [[ "$target" == *apple-ios* && "$BUILD_IOS" == "0" ]]; then
        continue
    fi
    if [[ "$target" == *android* && "$BUILD_ANDROID" == "0" ]]; then
        continue
    fi
    [[ -d "$TOOLCHAIN_BIN/../lib/rustlib/$target" ]] || die "Rust target is not installed: $target"
done

build_swift() {
    local target_dir="$BUILD_ROOT/target-swift"
    local dist="$BUILD_ROOT/ios-dist"
    local framework_root="$BUILD_ROOT/swift-frameworks"
    local device_lib="$target_dir/aarch64-apple-ios/release/libcdk_ffi_swift.a"
    local simulator_arm_lib="$target_dir/aarch64-apple-ios-sim/release/libcdk_ffi_swift.a"
    local simulator_x86_lib="$target_dir/x86_64-apple-ios/release/libcdk_ffi_swift.a"
    local simulator_lib="$BUILD_ROOT/libcdk_ffi_swift-simulator.a"
    local bindgen="$target_dir/release/uniffi-bindgen-swift"
    local device_framework="$framework_root/ios-device/CashuDevKitFFI.framework"
    local simulator_framework="$framework_root/ios-simulator/CashuDevKitFFI.framework"
    local xcode_dir="${DEVELOPER_DIR:-}"

    require_command xcodebuild
    require_command xcrun
    require_command lipo

    if [[ -z "$xcode_dir" ]]; then
        xcode_dir="$(xcode-select -p 2>/dev/null || true)"
    fi
    if [[ ! -d "$xcode_dir/Platforms/iPhoneOS.platform" && -d /Applications/Xcode.app/Contents/Developer ]]; then
        xcode_dir=/Applications/Xcode.app/Contents/Developer
    fi
    [[ -d "$xcode_dir/Platforms/iPhoneOS.platform" ]] \
        || die "a full Xcode developer directory was not found; set DEVELOPER_DIR"

    log "Building Swift native libraries"
    (
        cd "$SOURCE_ROOT"
        env \
            CARGO_TARGET_DIR="$target_dir" \
            DEVELOPER_DIR="$xcode_dir" \
            IPHONEOS_DEPLOYMENT_TARGET="$IOS_DEPLOYMENT_TARGET" \
            "$CARGO_BIN" build --manifest-path bindings/swift/rust/Cargo.toml \
                --release --target aarch64-apple-ios
        env \
            CARGO_TARGET_DIR="$target_dir" \
            DEVELOPER_DIR="$xcode_dir" \
            IPHONEOS_DEPLOYMENT_TARGET="$IOS_DEPLOYMENT_TARGET" \
            "$CARGO_BIN" build --manifest-path bindings/swift/rust/Cargo.toml \
                --release --target aarch64-apple-ios-sim
        env \
            CARGO_TARGET_DIR="$target_dir" \
            DEVELOPER_DIR="$xcode_dir" \
            IPHONEOS_DEPLOYMENT_TARGET="$IOS_DEPLOYMENT_TARGET" \
            "$CARGO_BIN" build --manifest-path bindings/swift/rust/Cargo.toml \
                --release --target x86_64-apple-ios
        env CARGO_TARGET_DIR="$target_dir" "$CARGO_BIN" build \
            --manifest-path bindings/swift/rust/Cargo.toml \
            --release --bin uniffi-bindgen-swift
    )

    strip -S "$device_lib"
    strip -S "$simulator_arm_lib"
    strip -S "$simulator_x86_lib"
    lipo -create "$simulator_arm_lib" "$simulator_x86_lib" -output "$simulator_lib"

    log "Generating the Swift package and XCFramework"
    mkdir -p "$device_framework" "$simulator_framework" "$dist/Sources/Cdk"
    cp "$device_lib" "$device_framework/CashuDevKitFFI"
    cp "$simulator_lib" "$simulator_framework/CashuDevKitFFI"
    cp "$SOURCE_ROOT/bindings/swift/resources/Info-iOS.plist" "$device_framework/Info.plist"
    cp "$SOURCE_ROOT/bindings/swift/resources/Info-iOSSimulator.plist" "$simulator_framework/Info.plist"

    "$bindgen" "$device_lib" "$device_framework/Headers" --headers
    "$bindgen" "$device_lib" "$device_framework/Modules" \
        --xcframework --modulemap --module-name CashuDevKitFFI \
        --modulemap-filename module.modulemap
    "$bindgen" "$simulator_arm_lib" "$simulator_framework/Headers" --headers
    "$bindgen" "$simulator_arm_lib" "$simulator_framework/Modules" \
        --xcframework --modulemap --module-name CashuDevKitFFI \
        --modulemap-filename module.modulemap
    "$bindgen" "$device_lib" "$dist/Sources/Cdk" --swift-sources
    perl -pi -e 's/canImport\(cdk_ffiFFI\)/canImport(CashuDevKitFFI)/g; s/import cdk_ffiFFI/import CashuDevKitFFI/g' \
        "$dist/Sources/Cdk/"*.swift

    env DEVELOPER_DIR="$xcode_dir" xcodebuild -create-xcframework \
        -framework "$device_framework" \
        -framework "$simulator_framework" \
        -output "$dist/CashuDevKitFFI.xcframework"
    cp "$OVERLAY_DIR/templates/ios/Package.swift" "$dist/Package.swift"
    cp "$OVERLAY_DIR/templates/ios/README.md" "$dist/README.md"

    log "Installing the local Swift package in the iOS wallet"
    mkdir -p "$WALLET_ROOT/ios/Packages/cdk-swift-local"
    rsync -a --delete "$dist/" "$WALLET_ROOT/ios/Packages/cdk-swift-local/"

    perl -0pi -e \
        's#\.package\(url: "https://github\.com/cashubtc/cdk-swift", exact: "[^"]+"\)#.package(path: "Packages/cdk-swift-local")#g; s/package: "cdk-swift"/package: "cdk-swift-local"/g' \
        "$WALLET_ROOT/ios/Package.swift"

    perl -0pi -e \
        's#720E47A42FEC721E00E02CD0 /\* XCRemoteSwiftPackageReference "cdk-swift" \*/ = \{\n\s*isa = XCRemoteSwiftPackageReference;\n\s*repositoryURL = "https://github\.com/cashubtc/cdk-swift";\n\s*requirement = \{\n\s*kind = exactVersion;\n\s*version = "[^"]+";\n\s*\};\n\s*\};#720E47A42FEC721E00E02CD0 /\* XCLocalSwiftPackageReference "cdk-swift-local" \*/ = {\n\t\t\tisa = XCLocalSwiftPackageReference;\n\t\t\trelativePath = "Packages/cdk-swift-local";\n\t\t};#g; s/XCRemoteSwiftPackageReference "cdk-swift"/XCLocalSwiftPackageReference "cdk-swift-local"/g' \
        "$WALLET_ROOT/ios/CashuWallet.xcodeproj/project.pbxproj"

    if rg -q 'github\.com/cashubtc/cdk-swift|XCRemoteSwiftPackageReference "cdk-swift"' \
        "$WALLET_ROOT/ios/Package.swift" "$WALLET_ROOT/ios/CashuWallet.xcodeproj/project.pbxproj"; then
        die "could not switch every iOS CDK dependency to the generated local package"
    fi
    rm -f "$WALLET_ROOT/ios/Package.resolved"

    if [[ "$VERIFY_WALLETS" == "1" ]]; then
        log "Building the iOS wallet with the generated Swift bindings"
        env DEVELOPER_DIR="$xcode_dir" xcodebuild \
            -project "$WALLET_ROOT/ios/CashuWallet.xcodeproj" \
            -scheme CashuWallet \
            -sdk iphonesimulator \
            -destination 'generic/platform=iOS Simulator' \
            CODE_SIGNING_ALLOWED=NO \
            build
    fi
}

find_android_ndk() {
    local candidate
    local sdk_root="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}}"

    for candidate in \
        "${ANDROID_NDK_HOME:-}" \
        "$sdk_root/ndk-bundle" \
        /opt/homebrew/share/android-ndk; do
        if [[ -n "$candidate" && -d "$candidate/toolchains/llvm/prebuilt" ]]; then
            printf '%s\n' "$candidate"
            return
        fi
    done

    if [[ -d "$sdk_root/ndk" ]]; then
        while IFS= read -r candidate; do
            if [[ -d "$candidate/toolchains/llvm/prebuilt" ]]; then
                printf '%s\n' "$candidate"
                return
            fi
        done < <(find "$sdk_root/ndk" -mindepth 1 -maxdepth 1 -type d | sort -r)
    fi
    return 1
}

build_android_target() {
    local target="$1"
    local abi="$2"
    local compiler="$3"
    local env_suffix="$4"
    local target_dir="$5"
    local ndk_bin="$6"
    local output_dir="$7"
    local cc_var="CC_$env_suffix"
    local ar_var="AR_$env_suffix"
    local upper_env_suffix
    local linker_var
    local library="$target_dir/$target/release/libcdk_ffi.so"

    upper_env_suffix="$(printf '%s' "$env_suffix" | tr '[:lower:]' '[:upper:]')"
    linker_var="CARGO_TARGET_${upper_env_suffix}_LINKER"

    export "$cc_var=$ndk_bin/$compiler"
    export "$ar_var=$ndk_bin/llvm-ar"
    export "$linker_var=$ndk_bin/$compiler"

    (
        cd "$SOURCE_ROOT"
        env \
            CARGO_TARGET_DIR="$target_dir" \
            RUSTFLAGS='-C link-arg=-Wl,-z,max-page-size=16384 -C link-arg=-Wl,-z,common-page-size=16384' \
            "$CARGO_BIN" build --manifest-path bindings/kotlin/rust/Cargo.toml \
                --release --target "$target"
    )

    "$ndk_bin/llvm-strip" --strip-all "$library"
    mkdir -p "$output_dir/src/main/jniLibs/$abi"
    cp "$library" "$output_dir/src/main/jniLibs/$abi/libcdk_ffi.so"

    if "$ndk_bin/llvm-objdump" -p "$library" | awk '
        /LOAD/ && $NF != "2**14" { bad = 1 }
        END { exit bad }
    '; then
        printf '16 KB ELF alignment verified: %s\n' "$abi"
    else
        die "Android library is not 16 KB aligned: $abi"
    fi
}

build_android() {
    local ndk_root
    local ndk_prebuilt
    local ndk_bin
    local target_dir="$BUILD_ROOT/target-android"
    local dist="$BUILD_ROOT/android-dist"
    local bindgen
    local host_library
    local generated
    local java_home="${JAVA_HOME:-}"

    ndk_root="$(find_android_ndk)" || die "Android NDK was not found; set ANDROID_NDK_HOME"
    ndk_prebuilt="$(find "$ndk_root/toolchains/llvm/prebuilt" -mindepth 1 -maxdepth 1 -type d | sort | head -n 1)"
    [[ -n "$ndk_prebuilt" ]] || die "Android NDK LLVM toolchain was not found"
    ndk_bin="$ndk_prebuilt/bin"
    [[ -x "$ndk_bin/llvm-strip" ]] || die "Android NDK llvm-strip was not found"

    log "Building Kotlin native libraries for all Android ABIs"
    build_android_target aarch64-linux-android arm64-v8a \
        "aarch64-linux-android${ANDROID_API}-clang" aarch64_linux_android \
        "$target_dir" "$ndk_bin" "$dist"
    build_android_target armv7-linux-androideabi armeabi-v7a \
        "armv7a-linux-androideabi${ANDROID_API}-clang" armv7_linux_androideabi \
        "$target_dir" "$ndk_bin" "$dist"
    build_android_target i686-linux-android x86 \
        "i686-linux-android${ANDROID_API}-clang" i686_linux_android \
        "$target_dir" "$ndk_bin" "$dist"
    build_android_target x86_64-linux-android x86_64 \
        "x86_64-linux-android${ANDROID_API}-clang" x86_64_linux_android \
        "$target_dir" "$ndk_bin" "$dist"

    log "Generating Kotlin bindings"
    (
        cd "$SOURCE_ROOT"
        env CARGO_TARGET_DIR="$target_dir" "$CARGO_BIN" build \
            --manifest-path bindings/kotlin/rust/Cargo.toml \
            --release --bin uniffi-bindgen --lib
    )
    bindgen="$target_dir/release/uniffi-bindgen"
    host_library="$target_dir/release/libcdk_ffi.dylib"
    if [[ ! -f "$host_library" ]]; then
        host_library="$target_dir/release/libcdk_ffi.so"
    fi
    [[ -f "$host_library" ]] || die "host CDK library for Kotlin generation was not built"

    mkdir -p "$BUILD_ROOT/kotlin-generated"
    "$bindgen" generate \
        --library "$host_library" \
        --language kotlin \
        --out-dir "$BUILD_ROOT/kotlin-generated" \
        --no-format
    generated="$BUILD_ROOT/kotlin-generated/org/cashudevkit/cdk_ffi.kt"
    [[ -f "$generated" ]] || die "UniFFI did not generate the expected Kotlin source"
    mkdir -p "$dist/src/main/java/org/cashudevkit"
    cp "$generated" "$dist/src/main/java/org/cashudevkit/cdk_ffi.kt"
    cp "$OVERLAY_DIR/templates/android/build.gradle.kts" "$dist/build.gradle.kts"
    cp "$OVERLAY_DIR/templates/android/README.md" "$dist/README.md"

    log "Installing the local Kotlin package in the Android wallet"
    mkdir -p "$WALLET_ROOT/android/cdk-local"
    rsync -a --delete "$dist/" "$WALLET_ROOT/android/cdk-local/"

    if ! rg -q 'include\(":cdk-local"\)' "$WALLET_ROOT/android/settings.gradle.kts"; then
        perl -0pi -e 's/include\(":app"\)/include(":app")\ninclude(":cdk-local")/' \
            "$WALLET_ROOT/android/settings.gradle.kts"
    fi
    perl -0pi -e 's/implementation\(libs\.cdk\.android\)/implementation(project(":cdk-local"))/g' \
        "$WALLET_ROOT/android/app/build.gradle.kts"
    if ! rg -q '^android-library[[:space:]]*=' "$WALLET_ROOT/android/gradle/libs.versions.toml"; then
        perl -0pi -e \
            's/(\[plugins\]\nandroid-application[^\n]*\n)/$1android-library = { id = "com.android.library", version.ref = "agp" }\n/' \
            "$WALLET_ROOT/android/gradle/libs.versions.toml"
    fi
    if ! rg -q 'alias\(libs\.plugins\.android\.library\) apply false' "$WALLET_ROOT/android/build.gradle.kts"; then
        perl -0pi -e \
            's/(alias\(libs\.plugins\.android\.application\) apply false\n)/$1    alias(libs.plugins.android.library) apply false\n/' \
            "$WALLET_ROOT/android/build.gradle.kts"
    fi

    rg -q 'implementation\(project\(":cdk-local"\)\)' \
        "$WALLET_ROOT/android/app/build.gradle.kts" \
        || die "could not switch Android to the generated local CDK module"

    if [[ "$VERIFY_WALLETS" == "1" ]]; then
        log "Building the Android wallet with the generated Kotlin bindings"
        if [[ ! -x "$java_home/bin/java" && -x '/Applications/Android Studio.app/Contents/jbr/Contents/Home/bin/java' ]]; then
            java_home='/Applications/Android Studio.app/Contents/jbr/Contents/Home'
        fi
        [[ -x "$java_home/bin/java" ]] \
            || die "a Java runtime was not found; set JAVA_HOME before building Android"
        (
            cd "$WALLET_ROOT/android"
            env JAVA_HOME="$java_home" PATH="$java_home/bin:$PATH" ./gradlew :app:assembleDebug
        )
    fi
}

if [[ "$BUILD_IOS" == "1" ]]; then
    build_swift
fi
if [[ "$BUILD_ANDROID" == "1" ]]; then
    build_android
fi

log "Mobile bindings updated successfully"
printf 'Wallet: %s\n' "$WALLET_ROOT"
if [[ "$BUILD_IOS" == "1" ]]; then
    printf 'Swift:  %s\n' "$WALLET_ROOT/ios/Packages/cdk-swift-local"
fi
if [[ "$BUILD_ANDROID" == "1" ]]; then
    printf 'Kotlin: %s\n' "$WALLET_ROOT/android/cdk-local"
fi
