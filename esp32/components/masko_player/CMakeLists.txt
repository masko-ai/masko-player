idf_component_register(SRCS "masko_player.c"
                       INCLUDE_DIRS "include"
                       REQUIRES masko_core masko_codec masko_display
                                freertos esp_timer heap)
