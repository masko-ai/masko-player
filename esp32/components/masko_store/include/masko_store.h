/**
 * @file masko_store.h
 * @brief One read interface over flash, PSRAM and SD so packs can live anywhere.
 *
 * The player only ever sees a `masko_source_t`. Swapping the backend does not
 * change the player.
 */
#ifndef MASKO_STORE_H
#define MASKO_STORE_H

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#include "esp_err.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef struct masko_source_t masko_source_t;

/**
 * @brief A readable pack. `data` stays valid until the next call on the source
 *        for non-mmapped backends, so copy anything you need to keep.
 */
typedef struct {
    const uint8_t *data;
    size_t         len;
} masko_blob_t;

typedef enum {
    MASKO_SRC_FLASH = 0,
    MASKO_SRC_PSRAM,
    MASKO_SRC_SD,
} masko_src_kind_t;

/**
 * @brief Map a whole flash partition. Frames are read straight from the
 *        mapping, so a pack costs no RAM.
 *
 * @warning Never write the partition while a pack from it is playing. Flash
 *          writes stall execution and will visibly hitch the animation.
 */
esp_err_t masko_src_flash(const char *partition_label, masko_source_t **out);

/** @brief Wrap an already-resident buffer, e.g. a pack preloaded into PSRAM. */
esp_err_t masko_src_memory(const void *buf, size_t len, masko_source_t **out);

/**
 * @brief Read a pack from a mounted FAT filesystem (an SDMMC card).
 *
 * @param[in] path      file path, e.g. "/sdcard/idle.loop.mkpk"
 * @param[in] cache_len bytes to keep resident; 0 caches the whole file in PSRAM.
 *                       A pack is sequential-read, so a full cache costs no
 *                       extra SD traffic but does cost PSRAM.
 */
esp_err_t masko_src_sd(const char *path, size_t cache_len, masko_source_t **out);

/**
 * @brief Fetch a named pack from this source.
 *
 * The blob points at mmap'd flash, or at PSRAM owned by the source, and stays
 * valid until the next masko_source_open() on the same source. Hand it to the
 * player; never free it yourself.
 */
esp_err_t masko_source_open(masko_source_t *s, const char *name, masko_blob_t *out);

/** @brief Drop the caller's reference to a blob. Frees no storage. */
void masko_source_close(masko_blob_t *b);

/** @brief Which backend a source uses; MASKO_SRC_PSRAM when `s` is NULL. */
masko_src_kind_t masko_source_kind(const masko_source_t *s);

/** @brief Release the source and everything it owns (mmap handle, PSRAM cache). */
void masko_src_free(masko_source_t *s);

#ifdef __cplusplus
}
#endif
#endif /* MASKO_STORE_H */