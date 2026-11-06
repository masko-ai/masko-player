/**
 * @file test_masko_graph_json.c
 * @brief Host tests for the MaskoAnimationConfig loader.
 *
 * Builds against the cJSON source that ships with ESP-IDF, so the loader is
 * covered on a desktop with no hardware and no IDF toolchain.
 *
 * The document below matches the hand-written graph in test_masko_graph.c, and
 * adds one numeric input plus one edge whose `videos[]` entry is an object
 * rather than a bare string.
 *
 *   nodes  idle, happy, wave
 *   edges  0 idle  -> idle  loop        idle.loop.mkpk
 *          1 idle  -> happy             happy.mkpk      when happy == true
 *          2 happy -> happy  loop       happy.loop.mkpk
 *          3 idle  -> wave              wave.mkpk       when wave == true
 *          4 wave  -> wave   loop       wave.loop.mkpk
 *          5 happy -> idle              never.mkpk      when happy && wave
 *          6 happy -> idle              lowenergy.mkpk  when energy == 3
 *   inputs happy=false, wave=false, energy=3
 */
#include <stdio.h>
#include <string.h>

#include "masko_graph.h"
#include "masko_graph_json.h"

static int failures, checks;

static void check(int cond, const char *what)
{
    checks++;
    if (!cond) failures++;
    printf("  [%s] %s\n", cond ? "PASS" : "FAIL", what);
}

static const char GOOD[] =
"{"
"  \"version\": 2,"
"  \"initialNode\": \"idle\","
"  \"autoPlay\": true,"
"  \"nodes\": [{\"id\": \"idle\"}, {\"id\": \"happy\"}, {\"id\": \"wave\"}],"
"  \"inputs\": ["
"    {\"name\": \"happy\",  \"type\": \"boolean\", \"default\": false},"
"    {\"name\": \"wave\",   \"type\": \"boolean\", \"default\": false},"
"    {\"name\": \"energy\", \"type\": \"number\",  \"default\": 3}"
"  ],"
"  \"edges\": ["
"    {\"source\": \"idle\", \"target\": \"idle\", \"isLoop\": true,"
"     \"duration\": 5000, \"videos\": [\"idle.loop.mkpk\"]},"
"    {\"source\": \"idle\", \"target\": \"happy\", \"isLoop\": false,"
"     \"duration\": 1200, \"videos\": [\"happy.mkpk\"],"
"     \"conditions\": [{\"input\": \"happy\", \"op\": \"==\", \"value\": true}]},"
"    {\"source\": \"happy\", \"target\": \"happy\", \"isLoop\": true,"
"     \"duration\": 4000, \"videos\": [\"happy.loop.mkpk\"]},"
"    {\"source\": \"idle\", \"target\": \"wave\", \"isLoop\": false,"
"     \"duration\": 900, \"videos\": [\"wave.mkpk\"],"
"     \"conditions\": [{\"input\": \"wave\", \"op\": \"==\", \"value\": true}]},"
"    {\"source\": \"wave\", \"target\": \"wave\", \"isLoop\": true,"
"     \"duration\": 3000, \"videos\": [\"wave.loop.mkpk\"]},"
"    {\"source\": \"happy\", \"target\": \"idle\", \"isLoop\": false,"
"     \"duration\": 800, \"videos\": [\"never.mkpk\"],"
"     \"conditions\": [{\"input\": \"happy\", \"op\": \"==\", \"value\": true},"
"                    {\"input\": \"wave\",  \"op\": \"==\", \"value\": true}]},"
"    {\"source\": \"happy\", \"target\": \"idle\", \"isLoop\": false,"
"     \"duration\": 500, \"videos\": [{\"src\": \"lowenergy.mkpk\"}],"
"     \"conditions\": [{\"input\": \"energy\", \"op\": \"==\", \"value\": 3}]}"
"  ]"
"}";

static const char *clip_of(const masko_step_t *s)
{
    return (s && s->edge && s->edge->clip) ? s->edge->clip : "(none)";
}

/* Every parsed string must point inside the one arena. This is the property
 * that breaks first if the measuring pass forgets to reserve a string. */
static int str_in_arena(const masko_graph_json_t *p, const char *s)
{
    if (s == NULL) {
        return 1;
    }
    return s >= p->arena && s < p->arena + p->arena_len;
}

static int arena_ok(const masko_graph_json_t *p)
{
    if (p->arena == NULL) {
        return 0;
    }
    if (!str_in_arena(p, p->initial_node)) return 0;
    for (int i = 0; i < p->nnodes; i++)
        if (!str_in_arena(p, p->node_ids[i])) return 0;
    for (int i = 0; i < p->ninputs; i++)
        if (!str_in_arena(p, p->names[i])) return 0;
    for (int i = 0; i < p->nedges; i++) {
        if (!str_in_arena(p, p->edge_src[i])) return 0;
        if (!str_in_arena(p, p->edge_dst[i])) return 0;
        if (!str_in_arena(p, p->edge_clip[i])) return 0;
    }
    for (int i = 0; i < p->nconds_total; i++)
        if (!str_in_arena(p, p->cond_input[i])) return 0;
    return 1;
}

int main(void)
{
    printf("MaskoAnimationConfig loader\n");

    masko_graph_json_t pj;
    int rc = masko_graph_json_parse(GOOD, strlen(GOOD), &pj);
    check(rc == MASKO_GRAPH_OK, "parse a valid config");
    if (rc != MASKO_GRAPH_OK) {
        printf("  -> %s\n", masko_graph_strerror(rc));
        return 1;
    }

    check(pj.nnodes == 3 && pj.nedges == 7 && pj.ninputs == 3,
          "node, edge and input counts match the document");
    check(pj.nconds_total == 5, "all five conditions were collected");
    check(pj.autoplay, "autoPlay parsed");
    check(pj.edge_is_loop[0] && pj.edge_is_loop[2] && pj.edge_is_loop[4],
          "isLoop edges recognised");
    check(!pj.edge_is_loop[1] && !pj.edge_is_loop[6],
          "transition edges not marked as loops");
    check(arena_ok(&pj), "every parsed string points inside the arena");

    /* --- bind gives the runtime a usable graph ---------------------------- */
    masko_graph_t g;
    check(masko_graph_json_bind(&pj, &g) == MASKO_GRAPH_OK, "bind to masko_graph_t");
    check(g.nnodes == 3 && g.nedges == 7 && g.ninputs == 3,
          "bound view carries the counts");
    check(strcmp(g.initial_node, "idle") == 0, "initialNode carried through");

    masko_graph_rt_t rt;
    check(masko_graph_init(&rt, &g, NULL, NULL, 0) == MASKO_GRAPH_OK, "init runtime");
    masko_step_t s;

    /* --- defaults: nothing matches from idle, so the loop repeats --------- */
    masko_graph_next(&rt, &s);
    check(strcmp(clip_of(&s), "idle.loop.mkpk") == 0 && !s.is_transition,
          "the loop edge plays first");
    masko_graph_next(&rt, &s);
    check(strcmp(clip_of(&s), "idle.loop.mkpk") == 0 && !s.is_transition,
          "loop repeats while no condition holds");

    /* --- boolean transition, then landing --------------------------------- */
    masko_graph_set_bool(&rt, "happy", true);
    masko_graph_next(&rt, &s);
    check(strcmp(clip_of(&s), "happy.mkpk") == 0 && s.is_transition,
          "happy == true selects the happy transition");
    masko_graph_next(&rt, &s);
    check(strcmp(clip_of(&s), "happy.loop.mkpk") == 0 && !s.is_transition,
          "machine lands in the target node and plays its loop");

    /* --- numeric condition; this edge's videos[] entry is an object -------- */
    masko_graph_next(&rt, &s);
    check(strcmp(clip_of(&s), "lowenergy.mkpk") == 0 && s.is_transition,
          "energy == 3 (its default) takes the numeric edge");
    masko_graph_next(&rt, &s);
    check(strcmp(clip_of(&s), "idle.loop.mkpk") == 0 && !s.is_transition,
          "landing back in idle plays idle's loop");

    /* --- a numeric condition that does not hold falls back to the loop ----- */
    masko_graph_init(&rt, &g, NULL, NULL, 0);
    masko_graph_set_bool(&rt, "happy", true);
    masko_graph_set_number(&rt, "energy", 1);
    masko_graph_next(&rt, &s);              /* idle -> happy */
    masko_graph_next(&rt, &s);              /* happy.loop */
    masko_graph_set_bool(&rt, "happy", false);
    masko_graph_next(&rt, &s);
    check(strcmp(clip_of(&s), "happy.loop.mkpk") == 0 && !s.is_transition,
          "energy != 3 leaves the node looping");

    /* --- all conditions of an edge must hold ------------------------------ */
    masko_graph_init(&rt, &g, NULL, NULL, 0);
    masko_graph_set_bool(&rt, "happy", true);
    masko_graph_set_bool(&rt, "wave", true);
    masko_graph_set_number(&rt, "energy", 1);   /* keeps edge 6 out of the way */
    masko_graph_next(&rt, &s);
    check(strcmp(clip_of(&s), "happy.mkpk") == 0,
          "with happy and wave set, the earlier-declared edge wins");
    masko_graph_next(&rt, &s);                  /* happy.loop */
    masko_graph_next(&rt, &s);
    check(strcmp(clip_of(&s), "never.mkpk") == 0 && s.is_transition,
          "an edge with two conditions fires only when both hold");

    /* --- half of a two-condition edge is not enough ----------------------- */
    masko_graph_init(&rt, &g, NULL, NULL, 0);
    masko_graph_set_bool(&rt, "happy", true);
    masko_graph_set_bool(&rt, "wave", false);
    masko_graph_set_number(&rt, "energy", 1);
    masko_graph_next(&rt, &s);
    masko_graph_next(&rt, &s);
    masko_graph_next(&rt, &s);
    check(strcmp(clip_of(&s), "happy.loop.mkpk") == 0 && !s.is_transition,
          "one of two conditions satisfied does not fire the edge");

    /* --- the wave path ---------------------------------------------------- */
    masko_graph_init(&rt, &g, NULL, NULL, 0);
    masko_graph_set_bool(&rt, "wave", true);
    masko_graph_set_number(&rt, "energy", 1);
    masko_graph_next(&rt, &s);
    check(strcmp(clip_of(&s), "wave.mkpk") == 0 && s.is_transition,
          "wave == true selects the wave transition");
    masko_graph_next(&rt, &s);
    check(strcmp(clip_of(&s), "wave.loop.mkpk") == 0 && !s.is_transition,
          "lands in wave and plays wave.loop.mkpk");

    masko_graph_free(&rt);
    masko_graph_json_free(&pj);

    /* ---------------------------------------------------------------- errors */
    printf("\nRejections\n");

    struct { const char *name, *json; int want; } bad[] = {
        { "malformed JSON",
          "{ \"nodes\": [", MASKO_GRAPH_ERR_PARSE },
        { "top level is not an object",
          "[1, 2, 3]", MASKO_GRAPH_ERR_PARSE },
        { "missing initialNode",
          "{\"nodes\":[{\"id\":\"a\"}],\"edges\":[]}", MASKO_GRAPH_ERR_PARSE },
        { "initialNode is not a declared node",
          "{\"initialNode\":\"ghost\",\"nodes\":[{\"id\":\"a\"}],"
          "\"edges\":[{\"source\":\"a\",\"target\":\"a\",\"isLoop\":true,"
          "\"videos\":[\"a.mkpk\"]}]}", MASKO_GRAPH_ERR_NO_NODE },
        { "edge names an undeclared node",
          "{\"initialNode\":\"a\",\"nodes\":[{\"id\":\"a\"}],"
          "\"edges\":[{\"source\":\"a\",\"target\":\"a\",\"isLoop\":true,"
          "\"videos\":[\"a.mkpk\"]},"
          "{\"source\":\"a\",\"target\":\"ghost\",\"videos\":[\"b.mkpk\"]}]}",
          MASKO_GRAPH_ERR_NO_NODE },
        { "condition on an undeclared input",
          "{\"initialNode\":\"a\",\"nodes\":[{\"id\":\"a\"}],\"inputs\":[],"
          "\"edges\":[{\"source\":\"a\",\"target\":\"a\",\"isLoop\":true,"
          "\"videos\":[\"a.mkpk\"]},"
          "{\"source\":\"a\",\"target\":\"a\",\"videos\":[\"b.mkpk\"],"
          "\"conditions\":[{\"input\":\"ghost\",\"op\":\"==\","
          "\"value\":true}]}]}", MASKO_GRAPH_ERR_NO_INPUT },
        { "operator other than ==",
          "{\"initialNode\":\"a\",\"nodes\":[{\"id\":\"a\"}],"
          "\"inputs\":[{\"name\":\"mood\",\"type\":\"number\"}],"
          "\"edges\":[{\"source\":\"a\",\"target\":\"a\",\"isLoop\":true,"
          "\"videos\":[\"a.mkpk\"]},"
          "{\"source\":\"a\",\"target\":\"a\",\"videos\":[\"b.mkpk\"],"
          "\"conditions\":[{\"input\":\"mood\",\"op\":\">\",\"value\":3}]}]}",
          MASKO_GRAPH_ERR_PARSE },
        { "node with no loop edge",
          "{\"initialNode\":\"a\",\"nodes\":[{\"id\":\"a\"},{\"id\":\"b\"}],"
          "\"edges\":[{\"source\":\"a\",\"target\":\"a\",\"isLoop\":true,"
          "\"videos\":[\"a.mkpk\"]}]}", MASKO_GRAPH_ERR_NO_EDGE },
    };

    for (size_t i = 0; i < sizeof(bad) / sizeof(bad[0]); i++) {
        masko_graph_json_t p;
        const int got = masko_graph_json_parse(bad[i].json, strlen(bad[i].json), &p);
        check(got == bad[i].want, bad[i].name);
        if (got != bad[i].want) {
            printf("  -> wanted %s, got %s\n",
                   masko_graph_strerror(bad[i].want), masko_graph_strerror(got));
        }
        /* Every failure path must still be freeable. */
        masko_graph_json_free(&p);
    }

    /* Empty and NULL arguments. */
    masko_graph_json_t p2;
    check(masko_graph_json_parse(NULL, 0, &p2) == MASKO_GRAPH_ERR_ARG,
          "NULL text is refused");
    check(masko_graph_json_parse("{}", 2, NULL) == MASKO_GRAPH_ERR_ARG,
          "NULL output is refused");
    check(masko_graph_json_parse("", 0, &p2) == MASKO_GRAPH_ERR_ARG,
          "empty text is refused");

    printf("\n%s (%d checks, %d failure%s)\n",
           failures ? "FAILED" : "ALL TESTS PASSED",
           checks, failures, failures == 1 ? "" : "s");
    return failures ? 1 : 0;
}
