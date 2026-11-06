/**
 * @file masko_display.h
 * @brief Panel presenter: banded push of a bounding-box rectangle, plus an
 *        optional background fill for the region outside the bbox.
 *
 * Panel access is injected, so this component never links the Waveshare BSP and
 * can be pointed at any esp_lcd panel.
 */
#ifndef MASKO_DISPLAY_H
#define MASKO_DISPLAY_H

#include <stdint.h>
#include <stdbool.h>
#include "esp_err.h"
#include "esp_lcd_panel_ops.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
    esp_lcd_panel_handle_t panel;
    int      canvas_w;
    int      canvas_h;
    int      band_rows;      /* rows per esp_lcd draw call; 0 = MASKO default */
    bool     stats_enabled;
} masko_display_cfg_t;

typedef struct {
    masko_display_cfg_t cfg;
    int64_t last_push_us;
    int64_t max_push_us;
    uint32_t push_count;
} masko_display_t;

esp_err_t masko_display_new(const masko_display_cfg_t *cfg, masko_display_t **out);

/**
 * @brief Push a w×h RGB565 rectangle at (x, y), split into horizontal bands.
 *
 * A single call for the whole rectangle fails with ESP_ERR_NO_MEM on this board:
 * the SPI master allocates a private DMA buffer of max_transfer_sz from internal
 * DRAM, and a full frame is 434,312 B against ~311 KB free. Banding keeps each
 * transfer small; total bytes over the bus are unchanged.
 */
esp_err_t masko_display_blit(masko_display_t *d, int x, int y, int w, int h,
                              const void *rgb565);

/** @brief Fill a rectangle with a packed RGB565 colour, used to clear stale area. */
esp_err_t masko_display_fill(masko_display_t *d, int x, int y, int w, int h,
                              uint16_t rgb565);

void masko_display_free(masko_display_t *d);

#ifdef __cplusplus
}
#endif
#endif /* MASKO_DISPLAY_H */