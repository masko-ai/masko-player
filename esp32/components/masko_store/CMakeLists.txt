# masko_store: one read interface over flash, PSRAM and SD.
# The player only ever sees a masko_blob_t, so swapping backends costs nothing.
idf_component_register(SRCS "masko_store.c"
                       INCLUDE_DIRS "include"
                       REQUIRES masko_core heap esp_partition log)
