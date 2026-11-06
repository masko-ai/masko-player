/**
 * @file masko_graph.h
 * @brief Runtime for a Masko animation canvas: nodes, edges, inputs, conditions.
 *
 * This header and masko_graph.c contain NO ESP-IDF and NO JSON dependencies, so
 * the state machine can be exercised on a desktop. JSON parsing of the exported
 * `MaskoAnimationConfig` lives in masko_graph_json.c and needs cJSON.
 *
 * Behaviour (mirrors the canvas model):
 *   1. Start at `initial_node` and play its loop edge.
 *   2. At each loop boundary, evaluate that node's outgoing non-loop edges in
 *      order and take the first whose conditions all hold.
 *   3. A transition plays once, then the machine lands in the target node and
 *      plays that node's loop.
 *   4. If nothing matches, the loop repeats.
 */
#ifndef MASKO_GRAPH_H
#define MASKO_GRAPH_H

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

enum {
    MASKO_GRAPH_OK           = 0,
    MASKO_GRAPH_ERR_ARG      = -1,
    MASKO_GRAPH_ERR_PARSE    = -2,
    MASKO_GRAPH_ERR_NO_NODE  = -3,
    MASKO_GRAPH_ERR_NO_EDGE  = -4,
    MASKO_GRAPH_ERR_NO_INPUT = -5,
};

/** Masko documents only `==`; keep the enum open but do not invent operators. */
typedef enum {
    MASKO_OP_EQ = 0,
} masko_op_t;

typedef struct {
    const char *input;    /*!< name of the graph input this tests */
    masko_op_t  op;
    bool        is_number;/*!< compare against num_value, else bool_value */
    bool        bool_value;
    double      num_value;
} masko_cond_t;

typedef struct {
    const char        *source;      /*!< node id */
    const char        *target;      /*!< node id */
    bool               is_loop;     /*!< true when source == target */
    const char        *clip;        /*!< .mkpk filename to play */
    int32_t            duration_ms; /*!< advisory; the player uses frame counts */
    const masko_cond_t *conds;
    int                nconds;
} masko_edge_t;

typedef struct {
    const char *id;
} masko_node_t;

typedef struct {
    const char *name;
    bool        is_number;
    bool        bval;
    double      nval;
} masko_input_t;

typedef struct {
    const masko_node_t  *nodes;   int nnodes;
    const masko_edge_t  *edges;   int nedges;
    const masko_input_t *inputs;  int ninputs;
    const char          *initial_node;
    bool                 autoplay;
} masko_graph_t;

/** One clip to play, plus whether it is a one-shot transition. */
typedef struct {
    const masko_edge_t *edge;
    bool                is_transition;
} masko_step_t;

typedef struct {
    const masko_graph_t *g;

    int   current;          /*!< index into g->nodes */
    bool  landed;           /*!< set after a transition completes */

    /* Mutable input values. Caller-supplied pools are NOT freed by
     * masko_graph_free(); only the heap fallback is. */
    bool   *in_bool;
    double *in_num;
    bool    owns_inputs;

    uint32_t transitions_taken;
    uint32_t loops_repeated;
} masko_graph_rt_t;

/**
 * @brief Prepare a runtime. Copies input values so the caller may free the graph.
 * @param[out] rt      runtime state
 * @param[in]  in_pool scratch buffer for input values, or NULL to allocate
 * @param[in]  pool_sz size of in_pool, or 0
 */
int masko_graph_init(masko_graph_rt_t *rt, const masko_graph_t *g,
                     bool *in_pool, double *num_pool, size_t pool_sz);

void masko_graph_free(masko_graph_rt_t *rt);

int masko_graph_set_bool(masko_graph_rt_t *rt, const char *name, bool v);
int masko_graph_set_number(masko_graph_rt_t *rt, const char *name, double v);
bool masko_graph_get_bool(const masko_graph_rt_t *rt, const char *name, bool fallback);

/** @brief Node index of an id, or MASKO_GRAPH_ERR_NO_NODE. */
int masko_graph_find_node(const masko_graph_t *g, const char *id);

/**
 * @brief Decide which clip plays next.
 *
 * Advances the machine and reports the edge. After a transition the machine
 * lands in the target node and returns that node's loop edge next.
 */
int masko_graph_next(masko_graph_rt_t *rt, masko_step_t *out);

/** @brief Evaluate a single condition against the current input values. */
bool masko_cond_eval(const masko_graph_rt_t *rt, const masko_cond_t *c);

/** @brief First outgoing edge from `node` satisfying every condition. */
const masko_edge_t *masko_graph_match(const masko_graph_rt_t *rt, int node);

/** @brief The loop edge of a node (source == target), or NULL. */
const masko_edge_t *masko_graph_loop(const masko_graph_t *g, int node);

const char *masko_graph_strerror(int err);

#ifdef __cplusplus
}
#endif
#endif /* MASKO_GRAPH_H */