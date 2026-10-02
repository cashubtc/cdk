# Builds the generated C++ and links the Rust static library.
#
# Both halves are generated: `just nitro-bindings` writes cpp/generated, and
# `just nitro-android` writes the Rust libraries into src/main/jniLibs.
cmake_minimum_required(VERSION 3.22.1)
project(CashuNative)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(PACKAGE_ROOT ${CMAKE_CURRENT_SOURCE_DIR}/..)

file(GLOB GENERATED_SOURCES "${PACKAGE_ROOT}/cpp/generated/*.cpp")

add_library(CashuNative SHARED ${GENERATED_SOURCES})

target_include_directories(CashuNative PRIVATE "${PACKAGE_ROOT}/cpp/generated")

include(${PACKAGE_ROOT}/nitrogen/generated/android/CashuNative+autolinking.cmake)

add_library(cashu_ffi SHARED IMPORTED)
set_target_properties(cashu_ffi PROPERTIES
  IMPORTED_LOCATION
  "${PACKAGE_ROOT}/android/src/main/jniLibs/${ANDROID_ABI}/libcashu_ffi.so"
)

target_link_libraries(CashuNative cashu_ffi)
