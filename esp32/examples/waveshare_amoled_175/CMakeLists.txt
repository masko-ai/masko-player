cmake_minimum_required(VERSION 3.16)

# Must be set BEFORE project(): the component scanner reads it during configure.
# CMAKE_CURRENT_LIST_DIR is this file's directory, independent of the caller's cwd.
set(EXTRA_COMPONENT_DIRS "${CMAKE_CURRENT_LIST_DIR}/../../components")

include($ENV{IDF_PATH}/tools/cmake/project.cmake)
project(masko_amoled_175)