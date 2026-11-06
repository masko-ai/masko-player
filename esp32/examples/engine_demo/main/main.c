/*
 * The Masko player engine on an ESP32 board, without a screen: it loads a
 * canvas (spec/fixtures/canvases/otto.json), plays it on a simulated clock
 * where every clip lasts one second, turns behavior::working on, then
 * behavior::attention, and logs each clip the engine asks for and the heap
 * it takes. Flash it to any ESP32-S3 board and watch `idf.py monitor`.
 */
#include <stdio.h>
#include <string.h>
#include "esp_heap_caps.h"
#include "esp_log.h"
#include "esp_timer.h"
#include "masko_engine.h"

static const char *TAG = "masko_engine_demo";
extern const char otto_json_start[] asm("_binary_otto_json_start");
extern const char otto_json_end[] asm("_binary_otto_json_end");

/* The edge of the last showClip in a command list, copied into `edge`. */
static bool shown_edge(const char *commands, char *edge, size_t size)
{
    const char *found = NULL;
    for (const char *p = commands; (p = strstr(p, "\"showClip\"")) != NULL; p++) found = p;
    if (!found || !(found = strstr(found, "\"edge\":\""))) return false;
    found += 8;
    const char *end = strchr(found, '"');
    if (!end || (size_t)(end - found) >= size) return false;
    memcpy(edge, found, end - found);
    edge[end - found] = '\0';
    return true;
}

static void handle(const char *what, char *commands, char *playing, size_t size)
{
    ESP_LOGI(TAG, "%s -> %s", what, commands ? commands : "(null)");
    if (commands) shown_edge(commands, playing, size);
    masko_string_free(commands);
}

void app_main(void)
{
    const size_t before = heap_caps_get_free_size(MALLOC_CAP_DEFAULT);
    const int64_t t0 = esp_timer_get_time();
    char *error = NULL;
    masko_engine_t *engine = masko_engine_load(
        otto_json_start, otto_json_end - otto_json_start, &error);
    if (!engine) {
        ESP_LOGE(TAG, "refused: %s", error ? error : "?");
        masko_string_free(error);
        return;
    }
    ESP_LOGI(TAG, "loaded in %lld us, heap used %u bytes",
             (long long)(esp_timer_get_time() - t0),
             (unsigned)(before - heap_caps_get_free_size(MALLOC_CAP_DEFAULT)));

    char playing[96] = "";
    uint64_t now = 0;
    handle("start", masko_engine_start(engine, now), playing, sizeof playing);
    for (int clip = 0; clip < 12 && playing[0]; clip++) {
        handle("clip shown", masko_engine_clip_shown(engine, playing, now), playing, sizeof playing);
        if (clip == 3)
            handle("working on", masko_engine_set_bool(engine, "behavior::working", true, false, now), playing, sizeof playing);
        if (clip == 7)
            handle("attention on", masko_engine_set_bool(engine, "behavior::attention", true, false, now), playing, sizeof playing);
        now += 1000;
        char ended[96];
        strcpy(ended, playing);
        handle(ended, masko_engine_clip_ended(engine, ended, now), playing, sizeof playing);
    }
    ESP_LOGI(TAG, "heap in use after playing: %u bytes",
             (unsigned)(before - heap_caps_get_free_size(MALLOC_CAP_DEFAULT)));
    masko_engine_free(engine);
}
