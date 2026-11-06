/**
 * @file masko_toc.h
 * @brief Table of contents for a multi-pack bundle.
 *
 * A bundle lets one flash partition hold many clips, which is what a
 * multi-state mascot needs: an idle loop, a listening loop, a thinking loop,
 * a speaking loop. The TOC is a small header plus a fixed-size entry per
 * clip; the packs themselves are stored back to back, 512-byte aligned.
 *
 * Like masko_pack.c this translation unit has no ESP-IDF dependencies, so the
 * format can be tested on a desktop. See docs/FORMAT.md for the wire layout.
 */
#ifndef MASKO_TOC_H
#define MASKO_TOC_H

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

#define MASKO_TOC_MAGIC         "MKTC"
#define MASKO_TOC_FORMAT_VERSION 1u
#define MASKO_TOC_HEADER_SIZE   64u
#define MASKO_TOC_ENTRY_SIZE    64u
#define MASKO_TOC_NAME_SIZE     32u

enum {
    MASKO_TOC_OK            = 0,
    MASKO_TOC_ERR_ARG       = -1,
    MASKO_TOC_ERR_TOO_SMALL = -2,
    MASKO_TOC_ERR_MAGIC     = -3,
    MASKO_TOC_ERR_VERSION   = -4,
    MASKO_TOC_ERR_TRUNCATED = -5,
};

typedef struct {
    char     name[MASKO_TOC_NAME_SIZE];  /*!< NUL-padded, no extension */
    uint32_t offset;                     /*!< absolute within the bundle */
    uint32_t size;                       /*!< bytes */
    uint32_t crc32;                      /*!< crc32 of the pack bytes */
    uint16_t fps_num;
    uint16_t fps_den;
    uint32_t frame_count;
} masko_toc_entry_t;

typedef struct {
    uint32_t           format_version;
    uint32_t           entry_count;
    uint32_t           entries_offset;
    uint32_t           toc_size;      /*!< header + entries */
    uint32_t           total_size;    /*!< whole bundle */
    uint16_t           canvas_size;
    uint16_t           fps_num;
    uint16_t           fps_den;
    const uint8_t     *base;          /*!< start of the bundle */
    masko_toc_entry_t *entries;       /*!< entry_count entries */
    bool               owns_entries;
} masko_toc_t;

/**
 * @brief Parse a bundle's table of contents.
 *
 * @param[in]  base  Start of the bundle.
 * @param[in]  len   Length of the bundle in bytes.
 * @param[out] toc   Parsed TOC. `entries` points into `base`; it is not owned.
 *
 * @return MASKO_TOC_OK on success.
 */
int masko_toc_open(const void *base, size_t len, masko_toc_t *toc);

/** @brief Release anything the TOC owns. Safe to call on a zeroed TOC. */
void masko_toc_close(masko_toc_t *toc);

/** @brief Find a clip by name. Returns NULL when there is no such clip. */
const masko_toc_entry_t *masko_toc_find(const masko_toc_t *toc, const char *name);

/** @brief Index of a clip by name, or -1. */
int masko_toc_index(const masko_toc_t *toc, const char *name);

#ifdef __cplusplus
}
#endif
#endif /* MASKO_TOC_H */
