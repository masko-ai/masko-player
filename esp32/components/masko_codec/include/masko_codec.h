/**
 * @file masko_codec.h
 * @brief Baseline-JPEG → RGB565 wrapper around Espressif's esp_new_jpeg.
 */
#ifndef MASKO_CODEC_H
#define MASKO_CODEC_H

#include <stdint.h>
#include <stddef.h>
#include "esp_err.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef struct masko_decoder_t masko_decoder_t;

/**
 * @brief Create a decoder. One instance is reused for every frame.
 *
 * The decoder deliberately does NOT own an output buffer: with double buffering
 * the caller must choose the destination per frame.
 */
esp_err_t masko_decoder_new(uint16_t width, uint16_t height, masko_decoder_t **out);

/**
 * @brief Decode one baseline JPEG into RGB565.
 *
 * @param[in] jpeg     compressed frame bytes; may live in flash/PSRAM XIP memory
 * @param[in] len      frame length
 * @param[in] dst      16-byte-aligned RGB565 destination, >= masko_frame_bytes(w,h)
 * @param[in] dst_len  capacity of @p dst
 * @param[out] out     set to @p dst on success
 * @param[out] outw    received width
 * @param[out] outh    received height
 *
 * @return ESP_OK, ESP_ERR_INVALID_SIZE if the frame geometry does not match the
 *         buffer, or an esp_jpeg error otherwise.
 */
esp_err_t masko_decoder_decode(masko_decoder_t *d,
                               const uint8_t *jpeg, size_t len,
                               uint8_t *dst, size_t dst_len,
                               const uint8_t **out, uint16_t *outw, uint16_t *outh);

void masko_decoder_free(masko_decoder_t *d);

/** @brief Bytes one decoded frame occupies at w×h RGB565. */
size_t masko_frame_bytes(uint16_t w, uint16_t h);

#ifdef __cplusplus
}
#endif
#endif /* MASKO_CODEC_H */