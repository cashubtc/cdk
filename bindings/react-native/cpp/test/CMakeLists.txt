cmake_minimum_required(VERSION 3.20)
project(cashu_nitro_bridge_tests CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

if(NOT DEFINED CDK_TARGET_DIR)
  message(FATAL_ERROR "Pass -DCDK_TARGET_DIR=<cargo target profile dir>")
endif()

add_executable(bridge_tests
  ${CMAKE_CURRENT_SOURCE_DIR}/bridge_tests.cpp
  ${CMAKE_CURRENT_SOURCE_DIR}/../generated/CashuCryptoBridge.cpp
)

target_include_directories(bridge_tests PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../generated)

find_library(CASHU_FFI_LIBRARY
  NAMES cashu_ffi
  PATHS ${CDK_TARGET_DIR}
  NO_DEFAULT_PATH
  REQUIRED
)
target_link_libraries(bridge_tests PRIVATE ${CASHU_FFI_LIBRARY})
