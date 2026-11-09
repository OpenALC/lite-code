cmake_minimum_required(VERSION 3.5.0)
project(litecode_entry)

set(NATIVERENDER_ROOT_PATH ${CMAKE_CURRENT_SOURCE_DIR})

include_directories(${NATIVERENDER_ROOT_PATH}
                    ${NATIVERENDER_ROOT_PATH}/include)

# crates/lc-ohos is built by scripts/ohos/build.mjs (cargo build --release
# --target <triple>). Select the archive that matches the ABI hvigor asks for.
if(OHOS_ARCH STREQUAL "x86_64")
    set(LC_OHOS_TRIPLE x86_64-unknown-linux-ohos)
elseif(OHOS_ARCH STREQUAL "arm64-v8a" OR OHOS_ARCH STREQUAL "aarch64")
    set(LC_OHOS_TRIPLE aarch64-unknown-linux-ohos)
else()
    message(FATAL_ERROR "lc-ohos has no build for OHOS_ARCH=${OHOS_ARCH}")
endif()

get_filename_component(LC_REPO_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../../" ABSOLUTE)
set(LC_OHOS_LIB "${LC_REPO_ROOT}/target/${LC_OHOS_TRIPLE}/release/liblc_ohos.a")
if(NOT EXISTS "${LC_OHOS_LIB}")
    message(FATAL_ERROR
        "missing ${LC_OHOS_LIB}\n"
        "Run: cargo build --release --target ${LC_OHOS_TRIPLE} -p lc-ohos")
endif()

add_library(lc_ohos STATIC IMPORTED)
set_target_properties(lc_ohos PROPERTIES IMPORTED_LOCATION "${LC_OHOS_LIB}")

add_library(entry SHARED napi_init.cpp)
target_link_libraries(entry PUBLIC
    libace_napi.z.so
    libhilog_ndk.z.so
    lc_ohos
    m
    dl
    pthread)
