# masko_core: .mkpk reader and measured calibration constants.
# No ESP-IDF dependencies on purpose -- this must build and test on a desktop.
idf_component_register(SRCS "masko_pack.c"
                       INCLUDE_DIRS "include")
