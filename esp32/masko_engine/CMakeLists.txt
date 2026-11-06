# The Masko player engine (engine/, through bindings/c) as an ESP-IDF
# component: every canvas rule, the same code the web, Apple and Android
# players run. It builds the engine with Espressif's Rust toolchain, named
# `esp` (https://github.com/esp-rs/espup), into this build's own folder.
set(MASKO_ROOT "${CMAKE_CURRENT_LIST_DIR}/../..")
get_filename_component(MASKO_ROOT "${MASKO_ROOT}" ABSOLUTE)

# WHOLE_ARCHIVE: the linker reads the engine after this component, so the
# realpath() fallback must be in the link before anything asks for it.
idf_component_register(SRCS "masko_engine_libc.c"
                       INCLUDE_DIRS "${MASKO_ROOT}/bindings/c/include"
                       REQUIRES pthread
                       WHOLE_ARCHIVE)

if(IDF_TARGET STREQUAL "esp32s3")
    set(MASKO_RUST_TARGET xtensa-esp32s3-espidf)
elseif(IDF_TARGET STREQUAL "esp32")
    set(MASKO_RUST_TARGET xtensa-esp32-espidf)
elseif(IDF_TARGET STREQUAL "esp32s2")
    set(MASKO_RUST_TARGET xtensa-esp32s2-espidf)
else()
    message(FATAL_ERROR "masko_engine: no Rust target for ${IDF_TARGET} yet")
endif()

set(MASKO_RUST_DIR "${CMAKE_BINARY_DIR}/masko_engine_rust")
set(MASKO_ENGINE_LIB
    "${MASKO_RUST_DIR}/${MASKO_RUST_TARGET}/release/libmasko_player_c.a")
file(GLOB MASKO_RUST_SOURCES CONFIGURE_DEPENDS
     "${MASKO_ROOT}/engine/src/*.rs" "${MASKO_ROOT}/bindings/c/src/*.rs")

add_custom_command(
    OUTPUT "${MASKO_ENGINE_LIB}"
    COMMAND cargo +esp build -p masko-player-c --release
            --target ${MASKO_RUST_TARGET} -Zbuild-std=std,panic_abort
            --target-dir "${MASKO_RUST_DIR}"
    WORKING_DIRECTORY "${MASKO_ROOT}"
    DEPENDS ${MASKO_RUST_SOURCES}
            "${MASKO_ROOT}/engine/Cargo.toml" "${MASKO_ROOT}/bindings/c/Cargo.toml"
    COMMENT "Building the Masko player engine for ${MASKO_RUST_TARGET}"
    USES_TERMINAL VERBATIM)
add_custom_target(masko_engine_rust DEPENDS "${MASKO_ENGINE_LIB}")

add_library(masko_engine_rust_lib STATIC IMPORTED GLOBAL)
set_target_properties(masko_engine_rust_lib PROPERTIES
                      IMPORTED_LOCATION "${MASKO_ENGINE_LIB}")
add_dependencies(masko_engine_rust_lib masko_engine_rust)
target_link_libraries(${COMPONENT_LIB} PUBLIC masko_engine_rust_lib)
