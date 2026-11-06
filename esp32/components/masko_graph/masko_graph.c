/**
 * @file masko_graph.c
 * @brief Canvas state machine. No ESP-IDF, no JSON: fully host-testable.
 */
#include "masko_graph.h"

#include <stdlib.h>
#include <string.h>

static int streq(const char *a, const char *b)
{
    return (a && b) ? (strcmp(a, b) == 0) : 0;
}

int masko_graph_find_node(const masko_graph_t *g, const char *id)
{
    if (!g || !id) {
        return MASKO_GRAPH_ERR_NO_NODE;
    }
    for (int i = 0; i < g->nnodes; i++) {
        if (streq(g->nodes[i].id, id)) {
            return i;
        }
    }
    return MASKO_GRAPH_ERR_NO_NODE;
}

const char *masko_graph_strerror(int err)
{
    switch (err) {
    case MASKO_GRAPH_OK:           return "ok";
    case MASKO_GRAPH_ERR_ARG:      return "invalid argument";
    case MASKO_GRAPH_ERR_PARSE:    return "parse error";
    case MASKO_GRAPH_ERR_NO_NODE:  return "unknown node";
    case MASKO_GRAPH_ERR_NO_EDGE:  return "node has no outgoing edge";
    case MASKO_GRAPH_ERR_NO_INPUT: return "unknown input";
    default:                      return "unknown error";
    }
}

int masko_graph_init(masko_graph_rt_t *rt, const masko_graph_t *g,
                     bool *in_pool, double *num_pool, size_t pool_sz)
{
    if (!rt || !g || g->nnodes <= 0) {
        return MASKO_GRAPH_ERR_ARG;
    }

    memset(rt, 0, sizeof(*rt));
    rt->g = g;

    const int init = masko_graph_find_node(g, g->initial_node);
    if (init < 0) {
        return MASKO_GRAPH_ERR_NO_NODE;
    }
    rt->current = init;

    const size_t need = (size_t)g->ninputs;
    if (need > 0) {
        bool  *own_b = NULL;
        double *own_n = NULL;

        /* Callers that play for the lifetime of the graph can hand us pools to
         * avoid a heap allocation per init. */
        if (in_pool && num_pool && pool_sz >= need) {
            rt->in_bool = in_pool;
            rt->in_num  = num_pool;
            rt->owns_inputs = false;   /* caller keeps ownership */
        } else {
            own_b = (bool *)calloc(need, sizeof(bool));
            own_n = (double *)calloc(need, sizeof(double));
            if (!own_b || !own_n) {
                free(own_b); free(own_n);
                return MASKO_GRAPH_ERR_ARG;
            }
            rt->in_bool     = own_b;
            rt->in_num      = own_n;
            rt->owns_inputs = true;
        }

        for (int i = 0; i < g->ninputs; i++) {
            rt->in_bool[i] = g->inputs[i].bval;
            rt->in_num[i]  = g->inputs[i].is_number ? g->inputs[i].nval : 0.0;
        }
    }

    return MASKO_GRAPH_OK;
}

void masko_graph_free(masko_graph_rt_t *rt)
{
    if (!rt) {
        return;
    }
    /* Only the heap fallback is owned; caller pools are left untouched. */
    if (rt->owns_inputs) {
        free(rt->in_bool);
        free(rt->in_num);
        rt->in_bool = NULL;
        rt->in_num  = NULL;
        rt->owns_inputs = false;
    }
}

static int find_input(const masko_graph_rt_t *rt, const char *name)
{
    for (int i = 0; i < rt->g->ninputs; i++) {
        if (streq(rt->g->inputs[i].name, name)) {
            return i;
        }
    }
    return -1;
}

int masko_graph_set_bool(masko_graph_rt_t *rt, const char *name, bool v)
{
    if (!rt) {
        return MASKO_GRAPH_ERR_ARG;
    }
    const int i = find_input(rt, name);
    if (i < 0) {
        return MASKO_GRAPH_ERR_NO_INPUT;
    }
    rt->in_bool[i] = v;
    return MASKO_GRAPH_OK;
}

int masko_graph_set_number(masko_graph_rt_t *rt, const char *name, double v)
{
    if (!rt) {
        return MASKO_GRAPH_ERR_ARG;
    }
    const int i = find_input(rt, name);
    if (i < 0) {
        return MASKO_GRAPH_ERR_NO_INPUT;
    }
    rt->in_num[i] = v;
    return MASKO_GRAPH_OK;
}

bool masko_graph_get_bool(const masko_graph_rt_t *rt, const char *name, bool fallback)
{
    if (!rt) {
        return fallback;
    }
    const int i = find_input(rt, name);
    return (i < 0) ? fallback : rt->in_bool[i];
}

bool masko_cond_eval(const masko_graph_rt_t *rt, const masko_cond_t *c)
{
    if (!rt || !c || !c->input) {
        return false;
    }
    const int i = find_input(rt, c->input);
    if (i < 0) {
        /* An edge referencing an input the graph does not declare can never be
         * taken. Treating it as false keeps a malformed canvas from playing a
         * transition the author did not intend. */
        return false;
    }
    switch (c->op) {
    case MASKO_OP_EQ:
    default:
        return c->is_number ? (rt->in_num[i] == c->num_value)
                            : (rt->in_bool[i] == c->bool_value);
    }
}

const masko_edge_t *masko_graph_loop(const masko_graph_t *g, int node)
{
    if (!g || node < 0 || node >= g->nnodes) {
        return NULL;
    }
    const char *id = g->nodes[node].id;
    for (int i = 0; i < g->nedges; i++) {
        const masko_edge_t *e = &g->edges[i];
        if (e->is_loop || streq(e->source, e->target)) {
            if (streq(e->source, id)) {
                return e;
            }
        }
    }
    return NULL;
}

const masko_edge_t *masko_graph_match(const masko_graph_rt_t *rt, int node)
{
    if (!rt || node < 0 || node >= rt->g->nnodes) {
        return NULL;
    }
    const char *id = rt->g->nodes[node].id;

    /* Declaration order is the priority order. */
    for (int i = 0; i < rt->g->nedges; i++) {
        const masko_edge_t *e = &rt->g->edges[i];
        if (!streq(e->source, id)) {
            continue;
        }
        if (e->is_loop || streq(e->source, e->target)) {
            continue;                       /* loops are not transitions */
        }
        bool ok = true;
        for (int c = 0; c < e->nconds; c++) {
            if (!masko_cond_eval(rt, &e->conds[c])) {
                ok = false;
                break;
            }
        }
        if (ok) {
            return e;
        }
    }
    return NULL;
}

int masko_graph_next(masko_graph_rt_t *rt, masko_step_t *out)
{
    if (!rt || !out) {
        return MASKO_GRAPH_ERR_ARG;
    }

    /* Just finished a transition: land in the target node and play its loop. */
    if (rt->landed) {
        rt->landed = false;
        const masko_edge_t *loop = masko_graph_loop(rt->g, rt->current);
        if (loop == NULL) {
            return MASKO_GRAPH_ERR_NO_EDGE;
        }
        out->edge = loop;
        out->is_transition = false;
        return MASKO_GRAPH_OK;
    }

    /* At a loop boundary: try to take a transition. */
    const masko_edge_t *tr = masko_graph_match(rt, rt->current);
    if (tr != NULL) {
        const int target = masko_graph_find_node(rt->g, tr->target);
        if (target < 0) {
            return MASKO_GRAPH_ERR_NO_NODE;
        }
        rt->current = target;
        rt->landed = true;                  /* applied after this clip plays */
        rt->transitions_taken++;
        out->edge = tr;
        out->is_transition = true;
        return MASKO_GRAPH_OK;
    }

    const masko_edge_t *loop = masko_graph_loop(rt->g, rt->current);
    if (loop == NULL) {
        return MASKO_GRAPH_ERR_NO_EDGE;
    }
    rt->loops_repeated++;
    out->edge = loop;
    out->is_transition = false;
    return MASKO_GRAPH_OK;
}