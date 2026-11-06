/**
 * @file masko_display.c
 * @brief Banded panel push.
 */
#include "masko_display.h"

#include <stdlib.h>
#include <string.h>

#include "esp_log.h"
#include "esp_timer.h"
#include "esp_heap_caps.h"
#include "masko_calib.h"

static const char *TAG = "masko_display";

esp_err_t masko_display_new(const masko_display_cfg_t *cfg, masko_display_t **out)
{
    if (cfg == NULL || out == NULL || cfg->panel == NULL ||
        cfg->canvas_w <= 0 || cfg->canvas_h <= 0) {
        return ESP_ERR_INVALID_ARG;
    }

    masko_display_t *d = calloc(1, sizeof(*d));
    if (d == NULL) {
        return ESP_ERR_NO_MEM;
    }
    d->cfg = *cfg;
    if (d->cfg.band_rows <= 0) {
        d->cfg.band_rows = MASKO_PUSH_BAND_ROWS;
    }

    const int64_t band_bytes = (int64_t)cfg->canvas_w * d->cfg.band_rows * 2;
    ESP_LOGI(TAG, "%dx%d, %d-row bands = %d B per transfer (internal DMA budget %u)",
             cfg->canvas_w, cfg->canvas_h, d->cfg.band_rows, (int)band_bytes,
             (unsigned)MASKO_INTERNAL_DMA_BUDGET);
    if (band_bytes > (int)MASKO_INTERNAL_DMA_BUDGET) {
        ESP_LOGE(TAG, "band of %d B exceeds the internal DMA budget; lower band_rows",
                 (int)band_bytes);
        free(d);
        return ESP_ERR_INVALID_SIZE;
    }

    *out = d;
    return ESP_OK;
}

esp_err_t masko_display_blit(masko_display_t *d, int x, int y, int w, int h,
                              const void *rgb565)
{
    if (d == NULL || rgb565 == NULL || w <= 0 || h <= 0) {
        return ESP_ERR_INVALID_ARG;
    }
    if (x < 0 || y < 0 || x > d->cfg.canvas_w || y > d->cfg.canvas_h ||
        w > d->cfg.canvas_w - x || h > d->cfg.canvas_h - y) {
        return ESP_ERR_INVALID_ARG;
    }

    const int64_t t0 = esp_timer_get_time();
    const int bytes_per_row = w * 2;
    const uint8_t *src = (const uint8_t *)rgb565;

    for (int row = 0; row < h; row += d->cfg.band_rows) {
        const int rows = (h - row < d->cfg.band_rows) ? (h - row) : d->cfg.band_rows;
        const esp_err_t err = esp_lcd_panel_draw_bitmap(
            d->cfg.panel, x, y + row, x + w, y + row + rows,
            src + (size_t)row * bytes_per_row);
        if (err != ESP_OK) {
            /* Usually ESP_ERR_NO_MEM: band_rows is still too large. */
            ESP_LOGE(TAG, "band at y=%d (%dx%d) failed: %s",
                     y + row, w, rows, esp_err_to_name(err));
            return err;
        }
    }

    const int64_t dt = esp_timer_get_time() - t0;
    if (d->cfg.stats_enabled) {
        d->last_push_us = dt;
        if (dt > d->max_push_us) d->max_push_us = dt;
        d->push_count++;
    }
    return ESP_OK;
}

esp_err_t masko_display_fill(masko_display_t *d, int x, int y, int w, int h,
                              uint16_t rgb565)
{
    if (d == NULL || w <= 0 || h <= 0 || x < 0 || y < 0 ||
        x > d->cfg.canvas_w || y > d->cfg.canvas_h ||
        w > d->cfg.canvas_w - x || h > d->cfg.canvas_h - y) {
        return ESP_ERR_INVALID_ARG;
    }

    /* One strip, reused for every band. Internal DRAM is fine here: it is small
     * and short-lived, and it must not be PSRAM because the SPI DMA source for a
     * fill is cheaper to keep contiguous. */
    const int rows = d->cfg.band_rows < h ? d->cfg.band_rows : h;
    uint8_t *strip = heap_caps_malloc((size_t)w * rows * sizeof(uint16_t),
                                      MALLOC_CAP_INTERNAL | MALLOC_CAP_8BIT);
    if (strip == NULL) {
        return ESP_ERR_NO_MEM;
    }
    for (int i = 0; i < w * rows; i++) {
        strip[2 * i] = (uint8_t)(rgb565 >> 8);
        strip[2 * i + 1] = (uint8_t)rgb565;
    }

    esp_err_t err = ESP_OK;
    for (int row = 0; row < h && err == ESP_OK; row += rows) {
        const int n = (h - row < rows) ? (h - row) : rows;
        err = esp_lcd_panel_draw_bitmap(d->cfg.panel, x, y + row, x + w, y + row + n,
                                        (row == 0) ? strip : strip);
    }
    heap_caps_free(strip);
    return err;
}

void masko_display_free(masko_display_t *d)
{
    free(d);
}
