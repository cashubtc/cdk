cmake_minimum_required(VERSION 3.22)
project(CashuDevKit)

# Find react-native-nitro-modules
find_package(react-native-nitro-modules REQUIRED)

# Add the Nitro-generated specs + our implementation
add_library(CashuDevKit SHARED
  ../cpp/HybridOutputDataCreator.cpp
  # nitrogen-generated files will be added here after codegen
)

target_include_directories(CashuDevKit PRIVATE
  ../cpp
  ../nitrogen/generated/android
  ../nitrogen/generated/shared
)

# Link against nitro
target_link_libraries(CashuDevKit
  react-native-nitro-modules::NitroModules
)

# Link against pre-built cdk_ffi native library
# The .so must be placed at: android/src/main/jniLibs/<abi>/libcdk_ffi.so
find_library(CDK_FFI cdk_ffi PATHS
  ${CMAKE_SOURCE_DIR}/src/main/jniLibs/${ANDROID_ABI}
)
if(CDK_FFI)
  target_link_libraries(CashuDevKit ${CDK_FFI})
else()
  message(WARNING "libcdk_ffi.so not found for ${ANDROID_ABI} - build will fail at link time")
endif()
