/**
 * @file masko_graph_json.c
 * @brief cJSON loader for MaskoAnimationConfig 2.0.
 *
 * The state machine in masko_graph.c never sees JSON. This file turns a parsed
 * document into flat arrays plus a string arena, so the runtime can be handed a
 * plain const masko_graph_t and the whole graph costs one free().
 */
#include "masko_graph_json.h"

#include <stdlib.h>
#include <string.h>

#include "cJSON.h"

/* ------------------------------------------------------------------ arena */

typedef struct {
    char  *base;
    size_t used;
    size_t cap;
} arena_t;

/* With base == NULL this only accounts for the bytes, so the same traversal can
 * be run twice: once to size the arena, once to fill it. */
static char *ar_put(arena_t *a, const char *s)
{
    if (s == NULL) {
        return NULL;
    }
    const size_t n = strlen(s) + 1;
    if (a->base == NULL) {
        a->cap += n;
        return NULL;
    }
    char *p = a->base + a->used;
    memcpy(p, s, n);
    a->used += n;
    return p;
}

/* calloc sized by a count read from the document.
 *
 * The guard is n > 0, not n: casting a negative int straight to size_t wraps to
 * a huge value, which the toolchain rightly warns about, and a negative count
 * should mean "allocate nothing" rather than "allocate everything". */
static void *count_calloc(int n, size_t sz)
{
    return (n > 0) ? calloc((size_t)n, sz) : NULL;
}

/* --------------------------------------------------------------- accessors */

/* Exports have used both spellings over time; accept either. */
static const char *obj_str(const cJSON *o, const char *a, const char *b)
{
    const cJSON *it = a ? cJSON_GetObjectItemCaseSensitive(o, a) : NULL;
    if ((!it || !cJSON_IsString(it)) && b) {
        it = cJSON_GetObjectItemCaseSensitive(o, b);
    }
    return (it && cJSON_IsString(it)) ? it->valuestring : NULL;
}

static bool obj_bool(const cJSON *o, const char *a, bool fallback)
{
    const cJSON *it = cJSON_GetObjectItemCaseSensitive(o, a);
    if (!it) {
        return fallback;
    }
    if (cJSON_IsTrue(it))  return true;
    if (cJSON_IsFalse(it)) return false;
    return fallback;
}

static int32_t obj_num(const cJSON *o, const char *a, int32_t fallback)
{
    const cJSON *it = cJSON_GetObjectItemCaseSensitive(o, a);
    return (it && cJSON_IsNumber(it)) ? (int32_t)it->valuedouble : fallback;
}

static const char *node_id_of(const cJSON *n)
{
    return obj_str(n, "id", "nodeId");
}

/**
 * @brief The clip this edge plays.
 *
 * `videos` is an array, but a single edge has exactly one playable clip here: a
 * transition plays once, so picking a variant would be a guess. Accepts a bare
 * string or an object carrying one under a common key.
 */
static const char *edge_clip_of(const cJSON *e)
{
    const cJSON *v = cJSON_GetObjectItemCaseSensitive(e, "videos");
    if (!v && cJSON_IsString(cJSON_GetObjectItemCaseSensitive(e, "video"))) {
        v = cJSON_GetObjectItemCaseSensitive(e, "video");
    }
    if (!cJSON_IsArray(v)) {
        return NULL;
    }
    const cJSON *first = cJSON_GetArrayItem(v, 0);
    if (cJSON_IsString(first)) {
        return first->valuestring;
    }
    if (cJSON_IsObject(first)) {
        const char *s = obj_str(first, "src", "url");
        return s ? s : obj_str(first, "name", "path");
    }
    return NULL;
}

static bool is_num_type(const char *type)
{
    /* Only these spellings have appeared; anything else is a boolean, which is
     * the safer default because a misread number compares false and the edge
     * simply does not fire. */
    return type && (!strcmp(type, "number") || !strcmp(type, "float") ||
                    !strcmp(type, "int")   || !strcmp(type, "double"));
}

/* ------------------------------------------------------------------- parse */

static int parse_inputs(const cJSON *root, masko_graph_json_t *g, arena_t *ar,
                        bool fill)
{
    const cJSON *ins = cJSON_GetObjectItemCaseSensitive(root, "inputs");
    const int n = cJSON_IsArray(ins) ? cJSON_GetArraySize(ins) : 0;

    g->ninputs = n;
    if (!fill) {
        for (int i = 0; i < n; i++) {
            const cJSON *it = cJSON_GetArrayItem(ins, i);
            ar_put(ar, obj_str(it, "name", NULL));
            ar_put(ar, obj_str(it, "type", NULL));
        }
        return MASKO_GRAPH_OK;
    }

    g->names     = count_calloc(n, sizeof(char *));
    g->in_is_num = count_calloc(n, sizeof(bool));
    g->in_bval   = count_calloc(n, sizeof(bool));
    g->in_nval   = count_calloc(n, sizeof(double));
    if (n && (!g->names || !g->in_is_num || !g->in_bval || !g->in_nval)) {
        return MASKO_GRAPH_ERR_ARG;
    }

    for (int i = 0; i < n; i++) {
        const cJSON *it  = cJSON_GetArrayItem(ins, i);
        const char *name = obj_str(it, "name", NULL);
        if (name == NULL) {
            return MASKO_GRAPH_ERR_PARSE;
        }
        g->names[i]     = ar_put(ar, name);
        g->in_is_num[i] = is_num_type(obj_str(it, "type", NULL));

        const cJSON *def = cJSON_GetObjectItemCaseSensitive(it, "default");
        if (def) {
            if (cJSON_IsNumber(def)) {
                g->in_is_num[i] = true;
                g->in_nval[i]   = def->valuedouble;
            } else if (cJSON_IsTrue(def)) {
                g->in_bval[i] = true;
            } else if (cJSON_IsFalse(def)) {
                g->in_bval[i] = false;
            }
        }
    }
    return MASKO_GRAPH_OK;
}

static int parse_nodes(const cJSON *root, masko_graph_json_t *g, arena_t *ar,
                       bool fill)
{
    const cJSON *ns = cJSON_GetObjectItemCaseSensitive(root, "nodes");
    const int n = cJSON_IsArray(ns) ? cJSON_GetArraySize(ns) : 0;

    g->nnodes = n;
    if (!fill) {
        for (int i = 0; i < n; i++) {
            ar_put(ar, node_id_of(cJSON_GetArrayItem(ns, i)));
        }
        return MASKO_GRAPH_OK;
    }

    g->node_ids = count_calloc(n, sizeof(char *));
    if (n && !g->node_ids) {
        return MASKO_GRAPH_ERR_ARG;
    }
    for (int i = 0; i < n; i++) {
        const char *id = node_id_of(cJSON_GetArrayItem(ns, i));
        if (id == NULL) {
            return MASKO_GRAPH_ERR_PARSE;
        }
        g->node_ids[i] = ar_put(ar, id);
    }
    return MASKO_GRAPH_OK;
}

static int input_index(const masko_graph_json_t *g, const char *name)
{
    for (int i = 0; i < g->ninputs; i++) {
        if (g->names[i] && name && strcmp(g->names[i], name) == 0) {
            return i;
        }
    }
    return -1;
}

static int node_index(const masko_graph_json_t *g, const char *id)
{
    for (int i = 0; i < g->nnodes; i++) {
        if (g->node_ids[i] && id && strcmp(g->node_ids[i], id) == 0) {
            return i;
        }
    }
    return -1;
}

static int parse_edges(const cJSON *root, masko_graph_json_t *g, arena_t *ar,
                       bool fill)
{
    const cJSON *es = cJSON_GetObjectItemCaseSensitive(root, "edges");
    const int n = cJSON_IsArray(es) ? cJSON_GetArraySize(es) : 0;
    g->nedges = n;

    /* Count conditions up front so both passes allocate the same shapes. */
    int total = 0;
    for (int i = 0; i < n; i++) {
        const cJSON *cs = cJSON_GetObjectItemCaseSensitive(
            cJSON_GetArrayItem(es, i), "conditions");
        total += cJSON_IsArray(cs) ? cJSON_GetArraySize(cs) : 0;
        if (!fill) {
            const cJSON *e = cJSON_GetArrayItem(es, i);
            ar_put(ar, obj_str(e, "source", NULL));
            ar_put(ar, obj_str(e, "target", NULL));
            ar_put(ar, edge_clip_of(e));
            if (cJSON_IsArray(cs)) {
                for (int c = 0; c < cJSON_GetArraySize(cs); c++) {
                    ar_put(ar, obj_str(cJSON_GetArrayItem(cs, c), "input", NULL));
                }
            }
        }
    }
    g->nconds_total = total;

    if (!fill) {
        return MASKO_GRAPH_OK;
    }

    g->edge_src     = count_calloc(n, sizeof(char *));
    g->edge_dst     = count_calloc(n, sizeof(char *));
    g->edge_clip    = count_calloc(n, sizeof(char *));
    g->edge_dur     = count_calloc(n, sizeof(int32_t));
    g->edge_is_loop = count_calloc(n, sizeof(bool));
    g->cond_off     = count_calloc(n, sizeof(int));
    g->cond_n       = count_calloc(n, sizeof(int));
    g->cond_input   = count_calloc(total, sizeof(char *));
    g->conds        = count_calloc(total, sizeof(masko_cond_t));
    if (n && (!g->edge_src || !g->edge_dst || !g->edge_clip || !g->edge_dur ||
              !g->edge_is_loop || !g->cond_off || !g->cond_n)) {
        return MASKO_GRAPH_ERR_ARG;
    }
    if (total && (!g->cond_input || !g->conds)) {
        return MASKO_GRAPH_ERR_ARG;
    }

    int at = 0;
    for (int i = 0; i < n; i++) {
        const cJSON *e = cJSON_GetArrayItem(es, i);
        const char *src = obj_str(e, "source", NULL);
        const char *dst = obj_str(e, "target", NULL);
        if (src == NULL || dst == NULL) {
            return MASKO_GRAPH_ERR_PARSE;
        }
        /* Validate against the arrays parsed before edges. */
        if (node_index(g, src) < 0 || node_index(g, dst) < 0) {
            return MASKO_GRAPH_ERR_NO_NODE;
        }

        g->edge_src[i]     = ar_put(ar, src);
        g->edge_dst[i]     = ar_put(ar, dst);
        g->edge_clip[i]    = ar_put(ar, edge_clip_of(e));
        g->edge_dur[i]     = obj_num(e, "duration", 0);
        g->edge_is_loop[i] = obj_bool(e, "isLoop", false) ||
                             (strcmp(src, dst) == 0);

        const cJSON *cs = cJSON_GetObjectItemCaseSensitive(e, "conditions");
        const int nc = cJSON_IsArray(cs) ? cJSON_GetArraySize(cs) : 0;
        g->cond_off[i] = at;
        g->cond_n[i]   = nc;

        for (int c = 0; c < nc; c++, at++) {
            const cJSON *cj = cJSON_GetArrayItem(cs, c);
            const char *in  = obj_str(cj, "input", NULL);
            if (in == NULL) {
                return MASKO_GRAPH_ERR_PARSE;
            }
            if (input_index(g, in) < 0) {
                return MASKO_GRAPH_ERR_NO_INPUT;
            }

            /* Real exports only ever show `==`. Treating an unknown operator as
             * `==` would silently change which transition fires, so refuse it. */
            const char *op = obj_str(cj, "op", "==");
            if (op && strcmp(op, "==") && strcmp(op, "eq") && strcmp(op, "equals")) {
                return MASKO_GRAPH_ERR_PARSE;
            }

            g->cond_input[at] = ar_put(ar, in);
            g->conds[at].input = g->cond_input[at];
            g->conds[at].op    = MASKO_OP_EQ;

            const int idx = input_index(g, in);
            const cJSON *v = cJSON_GetObjectItemCaseSensitive(cj, "value");
            if (v && cJSON_IsNumber(v)) {
                g->conds[at].is_number = true;
                g->conds[at].num_value = v->valuedouble;
            } else {
                g->conds[at].is_number = g->in_is_num[idx];
                g->conds[at].num_value = g->in_nval[idx];
                g->conds[at].bool_value = cJSON_IsTrue(v);
            }
        }
    }

    /* Every node must be able to play something, or the machine stalls there
     * with ERR_NO_EDGE at runtime instead of telling the author now. */
    for (int i = 0; i < g->nnodes; i++) {
        bool has_loop = false;
        for (int e = 0; e < n && !has_loop; e++) {
            if (g->edge_is_loop[e] &&
                strcmp(g->edge_src[e], g->node_ids[i]) == 0) {
                has_loop = true;
            }
        }
        if (!has_loop) {
            return MASKO_GRAPH_ERR_NO_EDGE;
        }
    }

    return MASKO_GRAPH_OK;
}

int masko_graph_json_parse(const char *text, size_t len, masko_graph_json_t *out)
{
    if (!text || len == 0 || !out) {
        return MASKO_GRAPH_ERR_ARG;
    }
    memset(out, 0, sizeof(*out));

    cJSON *root = cJSON_ParseWithLength(text, len);
    if (root == NULL) {
        return MASKO_GRAPH_ERR_PARSE;
    }
    if (!cJSON_IsObject(root)) {
        cJSON_Delete(root);
        return MASKO_GRAPH_ERR_PARSE;
    }

    out->json = malloc(len + 1);
    if (!out->json) {
        cJSON_Delete(root);
        return MASKO_GRAPH_ERR_ARG;
    }
    memcpy(out->json, text, len);
    out->json[len] = '\0';

    const char *init = obj_str(root, "initialNode", "initial_node");
    if (init == NULL) {
        cJSON_Delete(root);
        return MASKO_GRAPH_ERR_PARSE;
    }
    /* Read before the tree goes away. */
    const bool autoplay = obj_bool(root, "autoPlay", true);

    arena_t ar = {0};

    /* Pass 1 counts strings; pass 2 copies them. Same traversal both times. */
    int rc = parse_nodes(root, out, &ar, false);
    if (rc == MASKO_GRAPH_OK) rc = parse_inputs(root, out, &ar, false);
    if (rc == MASKO_GRAPH_OK) rc = parse_edges(root, out, &ar, false);
    /* initialNode is copied last, so it has to be reserved first or the fill
     * pass runs off the end of the arena. */
    if (rc == MASKO_GRAPH_OK) ar_put(&ar, init);
    if (rc != MASKO_GRAPH_OK) {
        cJSON_Delete(root);
        return rc;
    }

    ar.base = malloc(ar.cap ? ar.cap : 1);
    if (!ar.base) {
        cJSON_Delete(root);
        return MASKO_GRAPH_ERR_ARG;
    }
    out->arena     = ar.base;
    out->arena_len = ar.cap;

    rc = parse_nodes(root, out, &ar, true);
    if (rc == MASKO_GRAPH_OK) rc = parse_inputs(root, out, &ar, true);
    if (rc == MASKO_GRAPH_OK) rc = parse_edges(root, out, &ar, true);
    if (rc == MASKO_GRAPH_OK) out->initial_node = ar_put(&ar, init);
    cJSON_Delete(root);

    if (rc != MASKO_GRAPH_OK) {
        return rc;
    }
    /* Compare the arena copy, not `init`: the cJSON tree is gone by now and
     * `init` points into freed memory. */
    if (out->initial_node == NULL || node_index(out, out->initial_node) < 0) {
        return MASKO_GRAPH_ERR_NO_NODE;
    }
    out->autoplay = autoplay;

    return MASKO_GRAPH_OK;
}

int masko_graph_json_bind(masko_graph_json_t *g, masko_graph_t *out)
{
    if (!g || !out || g->nnodes <= 0) {
        return MASKO_GRAPH_ERR_ARG;
    }

    masko_node_t  *nodes  = count_calloc(g->nnodes, sizeof(masko_node_t));
    masko_edge_t  *edges  = count_calloc(g->nedges, sizeof(masko_edge_t));
    masko_input_t *inputs = count_calloc(g->ninputs, sizeof(masko_input_t));
    if (!nodes || !edges || !inputs) {
        free(nodes); free(edges); free(inputs);
        return MASKO_GRAPH_ERR_ARG;
    }

    for (int i = 0; i < g->nnodes; i++) {
        nodes[i].id = g->node_ids[i];
    }
    for (int i = 0; i < g->ninputs; i++) {
        inputs[i].name      = g->names[i];
        inputs[i].is_number = g->in_is_num[i];
        inputs[i].bval      = g->in_bval[i];
        inputs[i].nval      = g->in_nval[i];
    }
    for (int i = 0; i < g->nedges; i++) {
        edges[i].source      = g->edge_src[i];
        edges[i].target      = g->edge_dst[i];
        edges[i].is_loop     = g->edge_is_loop[i];
        edges[i].clip        = g->edge_clip[i];
        edges[i].duration_ms = g->edge_dur[i];
        edges[i].conds       = g->nconds_total ? &g->conds[g->cond_off[i]] : NULL;
        edges[i].nconds      = g->cond_n[i];
    }

    out->nodes        = nodes;
    out->nnodes       = g->nnodes;
    out->edges        = edges;
    out->nedges       = g->nedges;
    out->inputs       = inputs;
    out->ninputs      = g->ninputs;
    out->initial_node = g->initial_node;
    out->autoplay     = g->autoplay;

    /* Remembered so _free can release the view; re-binding frees the old one. */
    free(g->view_nodes);
    free(g->view_edges);
    free(g->view_inputs);
    g->view_nodes  = nodes;
    g->view_edges  = edges;
    g->view_inputs = inputs;
    return MASKO_GRAPH_OK;
}

void masko_graph_json_free(masko_graph_json_t *g)
{
    if (!g) {
        return;
    }
    free(g->view_nodes);
    free(g->view_edges);
    free(g->view_inputs);
    free(g->node_ids);
    free(g->names);
    free(g->in_is_num);
    free(g->in_bval);
    free(g->in_nval);
    free(g->edge_src);
    free(g->edge_dst);
    free(g->edge_clip);
    free(g->edge_dur);
    free(g->edge_is_loop);
    free(g->cond_off);
    free(g->cond_n);
    free(g->cond_input);
    free(g->conds);
    free(g->json);
    free(g->arena);
    memset(g, 0, sizeof(*g));
}
