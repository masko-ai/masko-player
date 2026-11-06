/**
 * @file masko_codec.c
 * @brief esp_new_jpeg wrapper.
 */
#include "masko_codec.h"

#include <string.h>
#include "esp_log.h"
#include "esp_heap_caps.h"
#include "esp_jpeg_dec.h"
#include "masko_calib.h"

static const char *TAG = "masko_codec";

struct masko_decoder_t {
    uint16_t width;
    uint16_t height;
};

size_t masko_frame_bytes(uint16_t w, uint16_t h)
{
    return (size_t)w * h * sizeof(uint16_t);
}

esp_err_t masko_decoder_new(uint16_t width, uint16_t height, masko_decoder_t **out)
{
    if (out == NULL || width == 0 || height == 0) {
        return ESP_ERR_INVALID_ARG;
    }
    masko_decoder_t *d = calloc(1, sizeof(*d));
    if (d == NULL) {
        return ESP_ERR_NO_MEM;
    }
    d->width  = width;
    d->height = height;
    *out = d;
    ESP_LOGI(TAG, "decoder ready: %ux%u (destination buffer supplied per frame)",
             width, height);
    return ESP_OK;
}

esp_err_t masko_decoder_decode(masko_decoder_t *d,
                               const uint8_t *jpeg, size_t len,
                               uint8_t *dst, size_t dst_len,
                               const uint8_t **out, uint16_t *outw, uint16_t *outh)
{
    if (d == NULL || jpeg == NULL || len == 0 || dst == NULL) {
        return ESP_ERR_INVALID_ARG;
    }
    if (dst_len < masko_frame_bytes(d->width, d->height)) {
        return ESP_ERR_INVALID_SIZE;
    }

    jpeg_dec_config_t cfg = {
        /* Big-endian, despite the S3 being little-endian: esp_jpeg writes both
         * bytes into the buffer in memory order and the CO5300 consumes them in
         * that same order over QSPI, expecting the high byte first. _LE renders
         * a yellow background as light green. */
        .output_type  = JPEG_PIXEL_FORMAT_RGB565_BE,
        .scale        = {0, 0},
        .clipper      = {0, 0},
        .rotate       = JPEG_ROTATE_0D,
        .block_enable = false,   /* needs multiples of 8; a 336x432 bbox is not one */
    };

    jpeg_dec_handle_t dec = NULL;
    if (jpeg_dec_open(&cfg, &dec) != JPEG_ERR_OK) {
        return ESP_FAIL;
    }

    jpeg_dec_io_t io = {
        .inbuf     = (uint8_t *)(uintptr_t)jpeg,   /* never written through */
        .inbuf_len = (int)len,
        .outbuf    = dst,
        .out_size  = (int)dst_len,
    };

    jpeg_dec_header_info_t info = {0};
    esp_err_t err = ESP_FAIL;

    if (jpeg_dec_parse_header(dec, &io, &info) != JPEG_ERR_OK) {
        ESP_LOGE(TAG, "JPEG header parse failed");
        goto done;
    }
    if (info.width != d->width || info.height != d->height) {
        ESP_LOGE(TAG, "frame is %ux%u, buffer is %ux%u",
                 info.width, info.height, d->width, d->height);
        err = ESP_ERR_INVALID_SIZE;
        goto done;
    }
    if (jpeg_dec_process(dec, &io) != JPEG_ERR_OK) {
        ESP_LOGE(TAG, "JPEG decode failed");
        goto done;
    }

    if (out) *out = dst;
    if (outw) *outw = info.width;
    if (outh) *outh = info.height;
    err = ESP_OK;

done:
    jpeg_dec_close(dec);
    return err;
}

void masko_decoder_free(masko_decoder_t *d)
{
    if (d == NULL) {
        return;
    }
    free(d);
}