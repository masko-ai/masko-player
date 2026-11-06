/**
 * @file masko_store.c
 * @brief Flash / PSRAM / SD pack sources behind masko_source_t.
 */
#include "masko_store.h"

#include <stdio.h>
#include <string.h>
#include <stdlib.h>

#include "esp_log.h"
#include "esp_heap_caps.h"
#include "esp_partition.h"

static const char *TAG = "masko_store";

struct masko_source_t {
    masko_src_kind_t kind;

    /* flash. The mmap handle is a uint32_t, and 0 is a legitimate value, so
     * validity is tracked explicitly rather than by testing the handle. */
    const esp_partition_t *part;
    const void            *map;
    esp_partition_mmap_handle_t mmap_h;
    bool                        mapped;

    /* memory */
    const uint8_t *mem;
    size_t         mem_len;

    /* sd */
    char   *path;
    size_t  cache_len;
    void   *cache;          /* PSRAM, or NULL to read on demand */
};

void masko_source_close(masko_blob_t *b)
{
    if (b == NULL) {
        return;
    }
    b->data = NULL;
    b->len  = 0;
}

masko_src_kind_t masko_source_kind(const masko_source_t *s)
{
    return s ? s->kind : MASKO_SRC_PSRAM;
}

esp_err_t masko_src_flash(const char *label, masko_source_t **out)
{
    if (label == NULL || out == NULL) {
        return ESP_ERR_INVALID_ARG;
    }

    const esp_partition_t *part = esp_partition_find_first(
        ESP_PARTITION_TYPE_DATA, ESP_PARTITION_SUBTYPE_ANY, label);
    if (part == NULL) {
        ESP_LOGE(TAG, "no partition named '%s'", label);
        return ESP_ERR_NOT_FOUND;
    }

    masko_source_t *s = calloc(1, sizeof(*s));
    if (s == NULL) {
        return ESP_ERR_NO_MEM;
    }
    s->kind = MASKO_SRC_FLASH;
    s->part = part;

    esp_err_t err = esp_partition_mmap(part, 0, part->size, ESP_PARTITION_MMAP_DATA,
                                       &s->map, &s->mmap_h);
    if (err != ESP_OK) {
        ESP_LOGE(TAG, "mmap of '%s' failed: %s", label, esp_err_to_name(err));
        free(s);
        return err;
    }
    s->mapped = true;

    ESP_LOGI(TAG, "flash source '%s': %u bytes mapped", label, (unsigned)part->size);
    ESP_LOGW(TAG, "never write this partition while a pack from it is playing");
    *out = s;
    return ESP_OK;
}

esp_err_t masko_src_memory(const void *buf, size_t len, masko_source_t **out)
{
    if (buf == NULL || out == NULL || len == 0) {
        return ESP_ERR_INVALID_ARG;
    }
    masko_source_t *s = calloc(1, sizeof(*s));
    if (s == NULL) {
        return ESP_ERR_NO_MEM;
    }
    s->kind    = MASKO_SRC_PSRAM;
    s->mem     = (const uint8_t *)buf;
    s->mem_len = len;
    *out = s;
    return ESP_OK;
}

/**
 * @brief Read a pack from a mounted FAT filesystem (an SDMMC card).
 *
 * The pack is pulled into PSRAM in one read, so it must fit. Pass 0 to size the
 * buffer from the file, or a byte count to pre-allocate; a cache smaller than
 * the file is refused rather than silently truncated, because a short pack would
 * fail much later inside the frame walker with no useful message.
 *
 * @param[in] path      file path, e.g. "/sdcard/idle.loop.mkpk"
 * @param[in] cache_len bytes to pre-allocate in PSRAM; 0 = measure the file first
 */
esp_err_t masko_src_sd(const char *path, size_t cache_len, masko_source_t **out)
{
    if (path == NULL || out == NULL) {
        return ESP_ERR_INVALID_ARG;
    }

    masko_source_t *s = calloc(1, sizeof(*s));
    if (s == NULL) {
        return ESP_ERR_NO_MEM;
    }
    s->path = strdup(path);
    if (s->path == NULL) {
        free(s);
        return ESP_ERR_NO_MEM;
    }
    s->kind      = MASKO_SRC_SD;
    s->cache_len = cache_len;
    *out = s;
    return ESP_OK;
}

esp_err_t masko_source_open(masko_source_t *s, const char *name, masko_blob_t *out)
{
    if (s == NULL || out == NULL) {
        return ESP_ERR_INVALID_ARG;
    }
    out->data = NULL;
    out->len  = 0;

    switch (s->kind) {
    case MASKO_SRC_PSRAM:
        /* A memory source is exactly one pack; `name` is ignored. */
        out->data = s->mem;
        out->len  = s->mem_len;
        return ESP_OK;

    case MASKO_SRC_FLASH: {
        /* Offset from the start of the partition. For a single pack per
         * partition this is 0; for a TOC, parse the TOC and add the offset. */
        const uint8_t *base = (const uint8_t *)s->map;
        out->data = base;
        out->len  = s->part->size;
        ESP_LOGI(TAG, "flash: serving %s from partition '%s' (%u bytes)",
                 name ? name : "(unnamed)", s->part->label, (unsigned)s->part->size);
        return ESP_OK;
    }

    case MASKO_SRC_SD: {
        FILE *f = fopen(s->path, "rb");
        if (f == NULL) {
            ESP_LOGE(TAG, "cannot open %s (is the card mounted?)", s->path);
            return ESP_FAIL;
        }
        if (fseek(f, 0, SEEK_END) != 0) {
            fclose(f);
            return ESP_FAIL;
        }
        const long n = ftell(f);
        if (n <= 0 || fseek(f, 0, SEEK_SET) != 0) {
            fclose(f);
            ESP_LOGE(TAG, "%s is empty or not seekable", s->path);
            return ESP_FAIL;
        }

        /* A pack is read whole, so a cache smaller than the file would hand the
         * player a truncated pack. Fail here, loudly, instead of much later in
         * the frame walker. */
        if (s->cache_len != 0 && s->cache_len < (size_t)n) {
            fclose(f);
            ESP_LOGE(TAG, "cache of %u bytes is too small for %s (%ld bytes); "
                          "the whole pack must be resident",
                     (unsigned)s->cache_len, s->path, n);
            return ESP_ERR_INVALID_SIZE;
        }

        const size_t want = s->cache_len ? s->cache_len : (size_t)n;
        if (s->cache == NULL) {
            s->cache = heap_caps_malloc(want, MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT);
            if (s->cache == NULL) {
                fclose(f);
                ESP_LOGE(TAG, "no PSRAM for a %u byte pack from %s",
                         (unsigned)want, s->path);
                return ESP_ERR_NO_MEM;
            }
            s->cache_len = want;
        }

        const size_t got = fread(s->cache, 1, want, f);
        fclose(f);
        if (got != (size_t)n) {
            ESP_LOGE(TAG, "short read from %s: %u of %ld bytes",
                     s->path, (unsigned)got, n);
            return ESP_FAIL;
        }
        out->data = (const uint8_t *)s->cache;
        out->len  = got;
        ESP_LOGI(TAG, "sd: read %u bytes from %s into PSRAM",
                 (unsigned)got, s->path);
        return ESP_OK;
    }

    default:
        return ESP_ERR_INVALID_ARG;
    }
}

void masko_src_free(masko_source_t *s)
{
    if (s == NULL) {
        return;
    }
    if (s->mapped) {
        esp_partition_munmap(s->mmap_h);
    }
    if (s->cache) {
        heap_caps_free(s->cache);
    }
    free(s->path);
    free(s);
}