/**
 * The Masko player engine for C firmware (ESP32 boards): every canvas rule of
 * spec/rules.md, no video, no network, no screen. Calls that can change what
 * plays return the commands as a JSON array, in order, to free with
 * masko_string_free(), such as {"type":"showClip","edge":"...","kind":"move",
 * "rate":2.0}, {"type":"preload","edge":"..."} or {"type":"poseEntered",
 * "pose":"..."}: the engine's Command. Play time `now` is in milliseconds.
 * Not thread-safe: call from one task.
 */
#ifndef MASKO_ENGINE_H
#define MASKO_ENGINE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct masko_engine masko_engine_t;

/**
 * Reads a release delivery of `len` bytes (a final zero byte is ignored); NULL
 * when refused, with "code: detail" in *error to free with masko_string_free().
 */
masko_engine_t *masko_engine_load(const char *json, size_t len, char **error);
void masko_engine_free(masko_engine_t *engine);
void masko_string_free(char *s);

char *masko_engine_start(masko_engine_t *engine, uint64_t now);
char *masko_engine_reset(masko_engine_t *engine, uint64_t now);
char *masko_engine_set_bool(masko_engine_t *engine, const char *name, bool value, bool instant, uint64_t now);
char *masko_engine_set_number(masko_engine_t *engine, const char *name, double value, bool instant, uint64_t now);
char *masko_engine_fire(masko_engine_t *engine, const char *name, bool instant, uint64_t now);
char *masko_engine_advance(masko_engine_t *engine, uint64_t now);
char *masko_engine_clip_shown(masko_engine_t *engine, const char *edge, uint64_t now);
char *masko_engine_clip_ended(masko_engine_t *engine, const char *edge, uint64_t now);
char *masko_engine_clip_failed(masko_engine_t *engine, const char *edge, uint64_t now);

/** When the engine next needs masko_engine_advance(); false when nothing waits. */
bool masko_engine_next_wake(const masko_engine_t *engine, uint64_t *out);

#ifdef __cplusplus
}
#endif

#endif
