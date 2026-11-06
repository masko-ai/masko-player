# The Masko player engine (engine/, through bindings/c) as an ESP-IDF
# component: every canvas rule, the same code the web, Apple and Android
# players run. The engine is built without Rust's standard library, so it
# needs nothing from the firmware but malloc, memalign, free and abort.
#
# A compiled engine for the chip in lib/<chip>/libmasko_player_c.a is used as
# it is, and the firmware builds without Rust. Without one, the component
# compiles the engine from this repository with Rust, into the build folder.
set(MASKO_ROOT "${CMAKE_CURRENT_LIST_DIR}/../..")
get_filename_component(MASKO_ROOT "${MASKO_ROOT}" ABSOLUTE)

# A packaged component carries the header in include/; in the repository it
# is bindings/c's.
if(EXISTS "${CMAKE_CURRENT_LIST_DIR}/include/masko_engine.h")
    set(MASKO_INCLUDE "include")
else()
    set(MASKO_INCLUDE "${MASKO_ROOT}/bindings/c/include")
endif()
idf_component_register(INCLUDE_DIRS "${MASKO_INCLUDE}")

set(MASKO_PREBUILT "${CMAKE_CURRENT_LIST_DIR}/lib/${IDF_TARGET}/libmasko_player_c.a")
if(EXISTS "${MASKO_PREBUILT}")
    set(MASKO_ENGINE_LIB "${MASKO_PREBUILT}")
    message(STATUS "masko_engine: compiled engine for ${IDF_TARGET}")
else()
    # Xtensa chips need Espressif's Rust (espup, toolchain `esp`) and its
    # copy of core; RISC-V chips use a stable Rust target.
    if(IDF_TARGET STREQUAL "esp32s3")
        set(MASKO_RUST_TARGET xtensa-esp32s3-none-elf)
    elseif(IDF_TARGET STREQUAL "esp32")
        set(MASKO_RUST_TARGET xtensa-esp32-none-elf)
    elseif(IDF_TARGET STREQUAL "esp32s2")
        set(MASKO_RUST_TARGET xtensa-esp32s2-none-elf)
    elseif(IDF_TARGET STREQUAL "esp32c3")
        set(MASKO_RUST_TARGET riscv32imc-unknown-none-elf)
    elseif(IDF_TARGET STREQUAL "esp32c6" OR IDF_TARGET STREQUAL "esp32h2")
        set(MASKO_RUST_TARGET riscv32imac-unknown-none-elf)
    else()
        message(FATAL_ERROR
            "masko_engine: no compiled engine and no Rust target for ${IDF_TARGET}")
    endif()
    if(MASKO_RUST_TARGET MATCHES "^xtensa")
        set(MASKO_CARGO cargo +esp build -Zbuild-std=core,alloc)
    else()
        set(MASKO_CARGO cargo build)
    endif()

    set(MASKO_RUST_DIR "${CMAKE_BINARY_DIR}/masko_engine_rust")
    set(MASKO_ENGINE_LIB
        "${MASKO_RUST_DIR}/${MASKO_RUST_TARGET}/release/libmasko_player_c.a")
    file(GLOB MASKO_RUST_SOURCES CONFIGURE_DEPENDS
         "${MASKO_ROOT}/engine/src/*.rs" "${MASKO_ROOT}/bindings/c/src/*.rs")
    add_custom_command(
        OUTPUT "${MASKO_ENGINE_LIB}"
        COMMAND ${MASKO_CARGO} -p masko-player-c --release --no-default-features
                --target ${MASKO_RUST_TARGET} --target-dir "${MASKO_RUST_DIR}"
        WORKING_DIRECTORY "${MASKO_ROOT}"
        DEPENDS ${MASKO_RUST_SOURCES}
                "${MASKO_ROOT}/engine/Cargo.toml" "${MASKO_ROOT}/bindings/c/Cargo.toml"
        COMMENT "Building the Masko player engine for ${MASKO_RUST_TARGET}"
        USES_TERMINAL VERBATIM)
    add_custom_target(masko_engine_rust DEPENDS "${MASKO_ENGINE_LIB}")
endif()

add_library(masko_engine_lib STATIC IMPORTED GLOBAL)
set_target_properties(masko_engine_lib PROPERTIES IMPORTED_LOCATION "${MASKO_ENGINE_LIB}")
if(TARGET masko_engine_rust)
    add_dependencies(masko_engine_lib masko_engine_rust)
endif()
target_link_libraries(${COMPONENT_LIB} INTERFACE masko_engine_lib)
