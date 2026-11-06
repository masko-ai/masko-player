cmake_minimum_required(VERSION 3.16)
# Only the engine: the .mkpk components in ../../components are not needed here.
set(EXTRA_COMPONENT_DIRS "${CMAKE_CURRENT_LIST_DIR}/../../masko_engine")
include($ENV{IDF_PATH}/tools/cmake/project.cmake)
project(masko_engine_demo)
