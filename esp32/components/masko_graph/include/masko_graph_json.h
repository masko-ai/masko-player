/**
 * @file masko_graph_json.h
 * @brief Load an exported MaskoAnimationConfig into a masko_graph_t.
 *
 * Split out from masko_graph.h so the state machine stays free of cJSON and
 * builds on a desktop with no JSON dependency at all.
 *
 * Source schema (MaskoAnimationConfig 2.0):
 *   nodes[]      { id }
 *   edges[]      { source, target, isLoop, duration, videos[], conditions[] }
 *   inputs[]     { name, type, default }
 *   initialNode, autoPlay
 *
 * Only `==` appears in real exports, so only MASKO_OP_EQ is accepted; any other
 * operator is a parse error rather than a silent pass, because a condition that
 * never fires is indistinguishable from one that was meant to.
 */
#ifndef MASKO_GRAPH_JSON_H
#define MASKO_GRAPH_JSON_H

#include "masko_graph.h"

#ifdef __cplusplus
extern "C" {
#endif

/** @brief Everything the parsed graph owns. Free with masko_graph_json_free. */
typedef struct {
    char     *json;      /*!< the original text, for diagnostics */

    char    **node_ids;  int nnodes;
    char    **names;     int ninputs;
    bool     *in_is_num;
    bool     *in_bval;
    double   *in_nval;
    const char *initial_node;
    bool       autoplay;
    int        nedges;

    /* Edge strings live in one arena so freeing is a handful of pointers. */
    char     *arena;     size_t arena_len;
    char    **edge_src;  char **edge_dst;  char **edge_clip;
    int32_t  *edge_dur;
    bool     *edge_is_loop;

    /* Conditions, flattened: edge e owns conds[cond_off[e] .. cond_off[e]+n). */
    int      *cond_off;
    int      *cond_n;
    int       nconds_total;
    masko_cond_t *conds;
    char    **cond_input;

    /* Scratch allocated by masko_graph_json_bind, released by _free. */
    void *view_nodes;
    void *view_edges;
    void *view_inputs;
} masko_graph_json_t;

/**
 * @brief Parse `len` bytes of JSON.
 *
 * On failure `out` is still safe to hand to masko_graph_json_free() -- a partial
 * parse owns partial allocations. Errors that mean the canvas is wrong are
 * reported rather than tolerated: an edge naming an undeclared node
 * (ERR_NO_NODE), a condition on an undeclared input (ERR_NO_INPUT), an operator
 * other than `==` (ERR_PARSE), or a node with no loop edge (ERR_NO_EDGE).
 */
int masko_graph_json_parse(const char *text, size_t len, masko_graph_json_t *out);

/**
 * @brief View a parsed graph as a const masko_graph_t for the runtime.
 * Valid until masko_graph_json_free(), or until the next bind, which frees the
 * previous view.
 */
int masko_graph_json_bind(masko_graph_json_t *g, masko_graph_t *out);

/** @brief Release everything a parsed graph owns, including any bound view. */
void masko_graph_json_free(masko_graph_json_t *g);

#ifdef __cplusplus
}
#endif
#endif /* MASKO_GRAPH_JSON_H */
