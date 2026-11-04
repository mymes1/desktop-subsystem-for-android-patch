# Copyright 2026 The DSA Authors
# SPDX-License-Identifier: Apache-2.0
#
# dsa-sensors for x86_64 Android, built with the NDK by build.sh, which fetches the pinned
# sources into SCRATCH_DIR and generates the binder code into AIDL_GEN_DIR first.
#
# It links nothing but the platform's C libraries (libc, libbinder_ndk, liblog): C++ is the
# NDK's, linked in statically, as for the power HAL. The framework's event queue is written
# directly (src/event_queue.cpp), since libfmq is not in the NDK.

cmake_minimum_required(VERSION 3.22)
project(dsa_sensors LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

if(NOT DEFINED SCRATCH_DIR OR NOT DEFINED AIDL_GEN_DIR)
    message(FATAL_ERROR "run build.sh, which sets SCRATCH_DIR and AIDL_GEN_DIR")
endif()

get_filename_component(REPO_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../.." ABSOLUTE)
set(BINDER_NDK "${SCRATCH_DIR}/binder_ndk/libs/binder/ndk")
set(PROTOCOL_CPP "${REPO_DIR}/dsa/protocol/cpp/generated")
# The platform calls a HAL service makes, which the NDK's libbinder_ndk.so leaves out; the
# hwcomposer's stand-in lists them.
set(PLATFORM_STUB "${REPO_DIR}/dsa/guest/hwcomposer/binder_ndk_platform_stub.c")

# protobuf-lite 3.21.12, for the generated sensor port codec.
set(protobuf_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(protobuf_BUILD_PROTOC_BINARIES OFF CACHE BOOL "" FORCE)
set(protobuf_BUILD_LIBPROTOC OFF CACHE BOOL "" FORCE)
set(protobuf_BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
set(protobuf_WITH_ZLIB OFF CACHE BOOL "" FORCE)
set(protobuf_INSTALL OFF CACHE BOOL "" FORCE)
set(protobuf_DISABLE_RTTI ON CACHE BOOL "" FORCE)
add_subdirectory("${SCRATCH_DIR}/protobuf-21.12" protobuf EXCLUDE_FROM_ALL)

# A link-time libbinder_ndk.so that defines every function the NDK's copy does, plus the
# platform calls. Only the names matter: on the device the binary loads the platform's own.
find_library(NDK_BINDER binder_ndk REQUIRED)
execute_process(
    COMMAND "${CMAKE_NM}" -D --defined-only --format=just-symbols "${NDK_BINDER}"
    OUTPUT_VARIABLE NDK_BINDER_SYMBOLS
    RESULT_VARIABLE NDK_BINDER_NM_RESULT)
if(NOT NDK_BINDER_NM_RESULT EQUAL 0 OR NDK_BINDER_SYMBOLS STREQUAL "")
    message(FATAL_ERROR "cannot list the symbols of ${NDK_BINDER}")
endif()
file(READ "${PLATFORM_STUB}" BINDER_STUB_SOURCE)
string(REGEX MATCHALL "[A-Za-z_][A-Za-z0-9_]*\\(" PLATFORM_CALLS "${BINDER_STUB_SOURCE}")
string(REPLACE "\n" ";" NDK_BINDER_SYMBOLS "${NDK_BINDER_SYMBOLS}")
set(BINDER_STUB "${CMAKE_CURRENT_BINARY_DIR}/binder_ndk_ndk_stub.c")
set(BINDER_STUB_TEXT "// Generated from ${NDK_BINDER}\n")
foreach(symbol IN LISTS NDK_BINDER_SYMBOLS)
    string(REGEX REPLACE "@.*$" "" symbol "${symbol}")
    string(STRIP "${symbol}" symbol)
    if(symbol STREQUAL "" OR "${symbol}(" IN_LIST PLATFORM_CALLS)
        continue()
    endif()
    string(APPEND BINDER_STUB_TEXT "void ${symbol}(void) {}\n")
endforeach()
file(WRITE "${BINDER_STUB}" "${BINDER_STUB_TEXT}")
add_library(binder_ndk_stub SHARED "${PLATFORM_STUB}" "${BINDER_STUB}")
set_target_properties(binder_ndk_stub PROPERTIES
    OUTPUT_NAME "binder_ndk"
    LIBRARY_OUTPUT_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/platform-stubs")
target_compile_options(binder_ndk_stub PRIVATE -Wno-builtin-requires-header -Wno-incompatible-library-redeclaration)

# The sensors HAL's generated binder code and that of the packages it imports.
file(GLOB_RECURSE SENSORS_AIDL_SOURCES CONFIGURE_DEPENDS "${AIDL_GEN_DIR}/src/android/hardware/sensors/*.cpp")
file(GLOB_RECURSE COMMON_AIDL_SOURCES CONFIGURE_DEPENDS "${AIDL_GEN_DIR}/src/android/hardware/common/*.cpp")

add_executable(dsa-sensors
    src/main.cpp
    src/sensors_service.cpp
    src/sensor_table.cpp
    src/sensor_messages.cpp
    src/event_queue.cpp
    src/host_link.cpp
    ${SENSORS_AIDL_SOURCES}
    ${COMMON_AIDL_SOURCES}
    ${PROTOCOL_CPP}/control.pb.cc
)

# DSA's own sources build without warnings; the fetched and generated ones are not ours to fix.
file(GLOB DSA_SENSORS_SOURCES CONFIGURE_DEPENDS src/*.cpp)
set_source_files_properties(${DSA_SENSORS_SOURCES} PROPERTIES COMPILE_OPTIONS "-Wall;-Wextra;-Werror")

target_include_directories(dsa-sensors PRIVATE src)
target_include_directories(dsa-sensors SYSTEM PRIVATE
    ${AIDL_GEN_DIR}/include
    ${BINDER_NDK}/include_cpp
    ${BINDER_NDK}/include_ndk
    ${BINDER_NDK}/include_platform
    ${PROTOCOL_CPP}
    ${SCRATCH_DIR}/protobuf-21.12/src
)
# Built as the vendor half of the system/vendor split, as the platform builds a vendor HAL:
# the binder objects carry vendor stability, and those of the VINTF interface VINTF
# stability, without which the framework would refuse to talk to it.
target_compile_definitions(dsa-sensors PRIVATE __ANDROID_VENDOR__ BINDER_STABILITY_SUPPORT)
target_compile_options(dsa-sensors PRIVATE
    -fvisibility=hidden
    -ffunction-sections
    -fdata-sections
    -Wno-unused-parameter
    -Wno-deprecated-declarations
)
target_link_options(dsa-sensors PRIVATE -Wl,--gc-sections -Wl,--as-needed -s)
target_link_libraries(dsa-sensors PRIVATE libprotobuf-lite binder_ndk_stub log)
