idf_component_register(SRCS "main.c"
                       REQUIRES masko_engine esp_timer
                       EMBED_TXTFILES "../../../../spec/fixtures/canvases/otto.json")
