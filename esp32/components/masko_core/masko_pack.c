/**
 * @file masko_pack.c
 * @brief `.mkpk` v1 reader. Deliberately free of ESP-IDF headers.
 */
#include "masko_pack.h"

#include <string.h>

/* ---------------------------------------------------------------- CRC-32 */
/* Standard reflected IEEE 802.3 polynomial, matching Python's zlib.crc32. */

static uint32_t crc32_table[256];
static int crc32_ready;

static void crc32_init(void)
{
    for (uint32_t i = 0; i < 256; i++) {
        uint32_t c = i;
        for (int k = 0; k < 8; k++) {
            c = (c & 1u) ? (0xEDB88320u ^ (c >> 1)) : (c >> 1);
        }
        crc32_table[i] = c;
    }
    crc32_ready = 1;
}

uint32_t masko_crc32(const uint8_t *data, size_t len, uint32_t seed)
{
    if (!crc32_ready) {
        crc32_init();
    }
    uint32_t c = seed ^ 0xFFFFFFFFu;
    for (size_t i = 0; i < len; i++) {
        c = crc32_table[(c ^ data[i]) & 0xFFu] ^ (c >> 8);
    }
    return c ^ 0xFFFFFFFFu;
}

/* ------------------------------------------------------------- decoding */

static uint16_t rd16(const uint8_t *p) { return (uint16_t)(p[0] | (p[1] << 8)); }
static uint32_t rd32(const uint8_t *p)
{
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) |
           ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}

static void decode_header(const uint8_t *b, masko_pack_header_t *h)
{
    h->format_version    = b[4];
    h->header_size       = rd16(b + 6);
    h->codec             = b[8];
    h->flags             = b[9];
    h->jpeg_subsampling  = b[10];
    h->reserved0         = b[11];

    h->canvas_w = rd16(b + 12);
    h->canvas_h = rd16(b + 14);
    h->bbox_x   = rd16(b + 16);
    h->bbox_y   = rd16(b + 18);
    h->bbox_w   = rd16(b + 20);
    h->bbox_h   = rd16(b + 22);
    h->fps_num  = rd16(b + 24);
    h->fps_den  = rd16(b + 26);

    h->frame_count = rd32(b + 28);
    h->index_offset = rd32(b + 32);
    h->data_offset  = rd32(b + 36);
    h->file_size    = rd32(b + 40);

    h->bg_rgb565  = rd16(b + 44);
    h->reserved1  = rd16(b + 46);

    h->payload_crc32 = rd32(b + 48);
    h->header_crc32  = rd32(b + 52);
}

int masko_pack_open(const void *base, size_t size, masko_pack_t *out)
{
    if (base == NULL || out == NULL) {
        return MASKO_ERR_ARG;
    }
    if (size < MASKO_HEADER_SIZE) {
        return MASKO_ERR_TOO_SMALL;
    }

    const uint8_t *b = (const uint8_t *)base;
    if (memcmp(b, MASKO_MAGIC, 4) != 0) {
        return MASKO_ERR_MAGIC;
    }
    if (b[4] != MASKO_FORMAT_VERSION) {
        return MASKO_ERR_VERSION;
    }

    decode_header(b, &out->hdr);

    if (out->hdr.header_size != MASKO_HEADER_SIZE) {
        return MASKO_ERR_VERSION;
    }
    /* header_crc32 covers bytes 0..51 */
    if (masko_crc32(b, 52, 0) != out->hdr.header_crc32) {
        return MASKO_ERR_HEADER_CRC;
    }
    if (out->hdr.file_size > size || out->hdr.file_size < MASKO_HEADER_SIZE) {
        return MASKO_ERR_TRUNCATED;
    }
    if (out->hdr.codec != MASKO_CODEC_JPEG_BASELINE) {
        return MASKO_ERR_CODEC;
    }

    /* Geometry must be usable without any runtime rounding. */
    if ((out->hdr.bbox_x & 1u) || (out->hdr.bbox_y & 1u) ||
        (out->hdr.bbox_w % MASKO_MCU) || (out->hdr.bbox_h % MASKO_MCU) ||
        out->hdr.bbox_w == 0 || out->hdr.bbox_h == 0) {
        return MASKO_ERR_GEOMETRY;
    }
    if ((uint32_t)out->hdr.bbox_x + out->hdr.bbox_w > out->hdr.canvas_w ||
        (uint32_t)out->hdr.bbox_y + out->hdr.bbox_h > out->hdr.canvas_h) {
        return MASKO_ERR_GEOMETRY;
    }

    if (out->hdr.frame_count == 0 || out->hdr.fps_num == 0 || out->hdr.fps_den == 0 ||
        out->hdr.index_offset < MASKO_HEADER_SIZE ||
        out->hdr.index_offset > out->hdr.file_size ||
        out->hdr.data_offset < out->hdr.index_offset ||
        out->hdr.data_offset > out->hdr.file_size ||
        (uint64_t)out->hdr.frame_count * 8u >
            (uint64_t)out->hdr.data_offset - out->hdr.index_offset) {
        return MASKO_ERR_TRUNCATED;
    }

    out->base  = b;
    out->size  = size;
    out->index = b + out->hdr.index_offset;
    return MASKO_OK;
}

int masko_pack_frame(const masko_pack_t *pack, uint32_t i,
                     const uint8_t **data, uint32_t *len)
{
    if (pack == NULL || data == NULL || len == NULL) {
        return MASKO_ERR_ARG;
    }
    if (i >= pack->hdr.frame_count) {
        return MASKO_ERR_ARG;
    }

    const uint8_t *e = pack->index + (size_t)i * 8u;
    const uint32_t off = rd32(e);
    const uint32_t n   = rd32(e + 4);

    if (off % MASKO_PAYLOAD_ALIGN != 0u) {
        return MASKO_ERR_GEOMETRY;
    }
    if (n == 0 || off < pack->hdr.data_offset || off > pack->hdr.file_size ||
        n > pack->hdr.file_size - off) {
        return MASKO_ERR_TRUNCATED;
    }

    *data = pack->base + off;
    *len  = n;
    return MASKO_OK;
}

float masko_pack_fps(const masko_pack_t *pack)
{
    if (pack == NULL || pack->hdr.fps_den == 0) {
        return 0.0f;
    }
    return (float)pack->hdr.fps_num / (float)pack->hdr.fps_den;
}

int masko_index_mjpeg(const uint8_t *data, size_t size,
                      uint32_t *offsets, int max_frames)
{
    if (data == NULL || offsets == NULL || max_frames <= 0) {
        return 0;
    }

    int count = 0;
    size_t i = 0;

    while (i + 1 < size && count < max_frames) {
        if (data[i] != 0xFFu || data[i + 1] != 0xD8u) {
            i++;                              /* inter-frame padding */
            continue;
        }

        const size_t start = i;
        int closed = 0;
        i += 2;                               /* consume SOI */

        while (i + 1 < size) {
            if (data[i] != 0xFFu) {
                i++;
                continue;
            }
            const uint8_t m = data[i + 1];

            if (m == 0xFFu) {                 /* fill byte */
                i++;
                continue;
            }
            if (m == 0x00u || (m >= 0xD0u && m <= 0xD7u)) {  /* stuffing / restart */
                i += 2;
                continue;
            }
            if (m == 0xD9u) {                 /* EOI closes the frame */
                i += 2;
                offsets[count++] = (uint32_t)start;
                closed = 1;
                break;
            }

            if (i + 3 >= size) {
                break;
            }
            const size_t seg = ((size_t)data[i + 2] << 8) | data[i + 3];
            if (seg < 2u || i + 2u + seg > size) {
                break;
            }
            i += 2u + seg;
        }

        if (!closed) {
            break;                           /* truncated tail */
        }
    }
    return count;
}

const char *masko_strerror(int err)
{
    switch (err) {
    case MASKO_OK:              return "ok";
    case MASKO_ERR_ARG:         return "invalid argument";
    case MASKO_ERR_TOO_SMALL:   return "file smaller than header";
    case MASKO_ERR_MAGIC:       return "bad magic (not a .mkpk file)";
    case MASKO_ERR_VERSION:     return "unsupported format version";
    case MASKO_ERR_HEADER_CRC:  return "header CRC mismatch";
    case MASKO_ERR_TRUNCATED:   return "truncated";
    case MASKO_ERR_CODEC:       return "unsupported codec";
    case MASKO_ERR_GEOMETRY:    return "invalid geometry";
    case MASKO_ERR_PAYLOAD_CRC: return "payload CRC mismatch";
    default:                    return "unknown error";
    }
}
