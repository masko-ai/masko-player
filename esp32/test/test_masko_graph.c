/**
 * @file test_masko_graph.c
 * @brief Host tests for the canvas state machine. No ESP-IDF, no JSON.
 *
 * Graph modelled on the Masko canvas:
 *
 *   nodes    idle, happy, wave
 *   edges    idle  --loop-->  idle.wave.mp4
 *            idle  --happy--> happy     when input happy == true
 *            happy --loop--> happy.jump.mp4
 *            idle  --wave-->  wave      when input wave == true
 *            wave  --loop--> wave.wave.mp4
 */
#include <stdio.h>
#include <string.h>

#include "masko_graph.h"

static int failures, checks;

static void check(int cond, const char *what)
{
    checks++;
    if (!cond) failures++;
    printf("  [%s] %s\n", cond ? "PASS" : "FAIL", what);
}

static const masko_node_t  NODES[]  = { {"idle"}, {"happy"}, {"wave"} };
static const masko_input_t INPUTS[] = {
    {"happy", false, false, 0.0},
    {"wave",  false, false, 0.0},
};

/* conditions */
static const masko_cond_t C_HAPPY[] = { {"happy", MASKO_OP_EQ, false, true,  0.0} };
static const masko_cond_t C_WAVE[]  = { {"wave",  MASKO_OP_EQ, false, true,  0.0} };
static const masko_cond_t C_NEVER[] = { {"happy", MASKO_OP_EQ, false, true, 0.0},
                                        {"wave",  MASKO_OP_EQ, false, true, 0.0} };

static const masko_edge_t EDGES[] = {
    { "idle",  "idle",  true,  "idle.loop.mp4",  5000, NULL,        0 },
    { "idle",  "happy", false, "happy.mp4",      1200, C_HAPPY,     1 },
    { "happy", "happy", true,  "happy.loop.mp4", 4000, NULL,        0 },
    { "idle",  "wave",  false, "wave.mp4",       900, C_WAVE,      1 },
    { "wave",  "wave",  true,  "wave.loop.mp4",  3000, NULL,        0 },
    { "happy", "idle",  false, "never.mp4",      800, C_NEVER,     2 },
};

static const masko_graph_t GRAPH = {
    NODES, 3, EDGES, 6, INPUTS, 2, "idle", true,
};

static masko_graph_rt_t rt;

static const char *step_clip(masko_step_t *s) { return s->edge ? s->edge->clip : "(none)"; }

int main(void)
{
    printf("Canvas state machine\n");

    int rc = masko_graph_init(&rt, &GRAPH, NULL, NULL, 0);
    check(rc == MASKO_GRAPH_OK, "init");

    /* --- happy path off: the loop repeats forever ------------------------- */
    masko_step_t s;
    rc = masko_graph_next(&rt, &s);
    check(rc == MASKO_GRAPH_OK && s.edge && strcmp(step_clip(&s), "idle.loop.mp4") == 0,
          "with no inputs set, the loop edge plays");
    check(!s.is_transition, "loop edge is not a transition");

    rc = masko_graph_next(&rt, &s);
    check(rc == MASKO_GRAPH_OK && strcmp(step_clip(&s), "idle.loop.mp4") == 0,
          "loop repeats without an input change");

    /* --- trigger happy ---------------------------------------------------- */
    check(masko_graph_set_bool(&rt, "happy", true) == MASKO_GRAPH_OK, "set input happy");
    rc = masko_graph_next(&rt, &s);
    check(rc == MASKO_GRAPH_OK && s.is_transition &&
          strcmp(step_clip(&s), "happy.mp4") == 0,
          "happy=true selects the happy transition");

    /* --- after the transition, land in the target node's loop -------------- */
    rc = masko_graph_next(&rt, &s);
    check(rc == MASKO_GRAPH_OK && !s.is_transition &&
          strcmp(step_clip(&s), "happy.loop.mp4") == 0,
          "machine lands in the target node and plays its loop");

    /* --- a node with no outgoing transition keeps looping ----------------- */
    rc = masko_graph_next(&rt, &s);
    check(rc == MASKO_GRAPH_OK && strcmp(step_clip(&s), "happy.loop.mp4") == 0,
          "happy has no reachable transition, so it loops");

    /* --- multi-condition edges never fire ---------------------------------- */
    check(rt.loops_repeated >= 2, "loop counter advanced");
    check(rt.transitions_taken == 1, "exactly one transition taken");

    /* --- return to idle and test the wave path ----------------------------- */
    masko_graph_free(&rt);   /* release the previous pools before re-init */
    masko_graph_init(&rt, &GRAPH, NULL, NULL, 0);
    masko_graph_set_bool(&rt, "wave", true);
    rc = masko_graph_next(&rt, &s);
    check(rc == MASKO_GRAPH_OK && s.is_transition &&
          strcmp(step_clip(&s), "wave.mp4") == 0,
          "wave=true selects the wave transition");

    rc = masko_graph_next(&rt, &s);
    check(rc == MASKO_GRAPH_OK && !s.is_transition &&
          strcmp(step_clip(&s), "wave.loop.mp4") == 0,
          "lands in wave and plays wave.loop.mp4");

    /* --- priority: first matching edge in declaration order wins ----------- */
    masko_graph_free(&rt);   /* release the previous pools before re-init */
    masko_graph_init(&rt, &GRAPH, NULL, NULL, 0);
    masko_graph_set_bool(&rt, "happy", true);
    masko_graph_set_bool(&rt, "wave", true);
    rc = masko_graph_next(&rt, &s);
    check(rc == MASKO_GRAPH_OK && strcmp(step_clip(&s), "happy.mp4") == 0,
          "with both inputs set, the earlier-declared edge wins");

    /* --- unknown input is an error, not a silent default ------------------ */
    check(masko_graph_set_bool(&rt, "nope", true) == MASKO_GRAPH_ERR_NO_INPUT,
          "setting an unknown input is refused");

    /* --- condition referencing an undeclared input can never fire --------- */
    masko_graph_free(&rt);   /* release the previous pools before re-init */
    masko_graph_init(&rt, &GRAPH, NULL, NULL, 0);
    const masko_cond_t ghost[] = { {"ghost", MASKO_OP_EQ, false, true, 0.0} };
    check(!masko_cond_eval(&rt, &ghost[0]),
          "condition on an undeclared input evaluates false");

    /* --- pools are caller-owned and not freed ----------------------------- */
    bool  pb[2] = {false, false};
    double pn[2] = {0.0, 0.0};
    masko_graph_free(&rt);   /* release the previous pools before re-init */
    rc = masko_graph_init(&rt, &GRAPH, pb, pn, 2);
    check(rc == MASKO_GRAPH_OK, "init with caller pools");
    masko_graph_set_bool(&rt, "wave", true);
    masko_graph_next(&rt, &s);
    check(pn[0] == 0.0 && pb[1] == true, "caller pool was written, not swapped");
    masko_graph_free(&rt);
    check(pb[1] == true, "caller pool survives masko_graph_free");

    /* --- bad graph -------------------------------------------------------- */
    masko_graph_t bad = GRAPH;
    bad.initial_node = "does_not_exist";
    check(masko_graph_init(&rt, &bad, NULL, NULL, 0) == MASKO_GRAPH_ERR_NO_NODE,
          "unknown initial node is refused");

    printf("\n%s (%d checks, %d failure%s)\n",
           failures ? "FAILED" : "ALL TESTS PASSED",
           checks, failures, failures == 1 ? "" : "s");
    return failures ? 1 : 0;
}