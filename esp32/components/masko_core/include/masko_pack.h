/**
 * @file masko_pack.h
 * @brief Reader for `.mkpk` v1 packs. No ESP-IDF dependencies: this translation
 *        unit compiles and runs on a desktop so the format can be tested without
 *        hardware. See docs/FORMAT.md for the wire layout.
 */
#ifndef MASKO_PACK_H
#define MASKO_PACK_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

#define MASKO_MAGIC          "MKPK"
#define MASKO_FORMAT_VERSION 1u
#define MASKO_HEADER_SIZE    64u
#define MASKO_PAYLOAD_ALIGN  512u
#define MASKO_MCU            16u   /* 4:2:0 MCU; bbox dims must be a multiple */

enum {
    MASKO_CODEC_JPEG_BASELINE = 1,
    MASKO_CODEC_RGB565_RAW    = 2,
    MASKO_CODEC_RGB565_DELTA  = 3,
};

enum {
    MASKO_FLAG_LOOP          = 1u << 0,
    MASKO_FLAG_ALPHA         = 1u << 1,
    MASKO_FLAG_PREMULTIPLIED = 1u << 2,
};

enum {
    MASKO_SUBSAMPLING_420 = 0,
    MASKO_SUBSAMPLING_422 = 1,
    MASKO_SUBSAMPLING_444 = 2,
    MASKO_SUBSAMPLING_GRAY = 3,
};

enum {
    MASKO_OK              = 0,
    MASKO_ERR_ARG         = -1,
    MASKO_ERR_TOO_SMALL   = -2,
    MASKO_ERR_MAGIC       = -3,
    MASKO_ERR_VERSION     = -4,
    MASKO_ERR_HEADER_CRC  = -5,
    MASKO_ERR_TRUNCATED   = -6,
    MASKO_ERR_CODEC       = -7,
    MASKO_ERR_GEOMETRY    = -8,
    MASKO_ERR_PAYLOAD_CRC = -9,
};

typedef struct {
    uint8_t  format_version;
    uint8_t  header_size;
    uint8_t  codec;
    uint8_t  flags;
    uint8_t  jpeg_subsampling;
    uint8_t  reserved0;

    uint16_t canvas_w, canvas_h;
    uint16_t bbox_x, bbox_y, bbox_w, bbox_h;
    uint16_t fps_num, fps_den;

    uint32_t frame_count;
    uint32_t index_offset;
    uint32_t data_offset;
    uint32_t file_size;
    uint16_t bg_rgb565;
    uint16_t reserved1;

    uint32_t payload_crc32;
    uint32_t header_crc32;
} masko_pack_header_t;

typedef struct {
    const uint8_t          *base;   /*< start of the pack, usually an mmap or a file read */
    size_t                  size;
    masko_pack_header_t     hdr;
    const uint8_t          *index;  /*< index_offset, for cheap frame lookup */
} masko_pack_t;

/** @brief Validate the header CRC and pack structure, then decode the header.
 * The payload CRC is available in the header for optional validation by callers. */
int masko_pack_open(const void *base, size_t size, masko_pack_t *out);

/**
 * @brief Locate frame @p i.
 * @param[out] data pointer to compressed frame bytes, valid while @p pack is
 * @param[out] len  frame length in bytes
 */
int masko_pack_frame(const masko_pack_t *pack, uint32_t i,
                     const uint8_t **data, uint32_t *len);

/** @brief Frames per second as a float, honouring the rational in the header. */
float masko_pack_fps(const masko_pack_t *pack);

/**
 * @brief Index a bare concatenated-JPEG stream without a pack header.
 *
 * Used for streaming sources (SD, HTTP) where frames arrive before a header is
 * known. Performs a full JPEG marker walk so a stray 0xFF inside entropy-coded
 * data cannot be mistaken for a marker.
 *
 * @param[in]  max_frames capacity of @p offsets, excluding the terminator entry
 * @return number of complete frames found
 */
int masko_index_mjpeg(const uint8_t *data, size_t size,
                      uint32_t *offsets, int max_frames);

/** @brief Recompute the payload CRC over index_offset..EOF. */
uint32_t masko_crc32(const uint8_t *data, size_t len, uint32_t seed);

const char *masko_strerror(int err);

#ifdef __cplusplus
}
#endif
#endif /* MASKO_PACK_H */
