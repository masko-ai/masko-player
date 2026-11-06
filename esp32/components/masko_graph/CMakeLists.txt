# masko_graph: canvas state machine + cJSON loader for MaskoAnimationConfig.
#
# masko_graph.c is deliberately free of ESP-IDF and JSON so the state machine can
# be compiled and tested on a desktop. masko_graph_json.c is the only part that
# needs cJSON; host tests build it against the cJSON source that ships with
# ESP-IDF, so both files stay covered without hardware.
idf_component_register(SRCS "masko_graph.c" "masko_graph_json.c"
                       INCLUDE_DIRS "include"
                       PRIV_REQUIRES cjson)
