/**
 * @file masko_calib.h
 * @brief Measured constants for the Waveshare ESP32-S3-Touch-AMOLED-1.75.
 *
 * Every value here was measured on real hardware, not estimated. See
 * docs/CALIBRATION.md for the raw numbers, the methodology, and where these
 * contradict the original SDK_PLAN.md estimates.
 *
 * Override any of these from the build if you port to different hardware.
 */
#ifndef MASKO_CALIB_H
#define MASKO_CALIB_H

/* ------------------------------------------------------------- panel --- */

/** CO5300 QSPI AMOLED, round. */
#define MASKO_PANEL_W              466
#define MASKO_PANEL_H              466

/**
 * Rows per esp_lcd_panel_draw_bitmap() call.
 *
 * Critical: the SPI master allocates a private DMA buffer of `max_transfer_sz`
 * from *internal* DRAM. A full frame is 466*466*2 = 434,312 B but only ~311 KB
 * of internal DRAM was free, so a whole-frame push fails with ESP_ERR_NO_MEM and
 * the screen stays black. 50 rows = 46,600 B per transfer is what the Waveshare
 * BSP's own LVGL path uses. Total bytes over the bus are unchanged.
 */
#define MASKO_PUSH_BAND_ROWS       50

/** Bytes for one band. Pass this as max_transfer_sz when initialising the panel. */
#define MASKO_PUSH_BAND_BYTES      (MASKO_PANEL_W * MASKO_PUSH_BAND_ROWS * 2)

/**
 * CO5300 output is RGB565 with the HIGH byte first.
 *
 * Use JPEG_PIXEL_FORMAT_RGB565_BE, not _LE. esp_jpeg writes both bytes into the
 * buffer in memory order and the panel consumes them in that order over QSPI.
 * With _LE the yellow background (238,206,57) renders as light green (131,251,115).
 */
#define MASKO_PANEL_RGB565_BIG_ENDIAN 1

/** Column address gap set by the BSP via esp_lcd_panel_set_gap(panel, 0x06, 0). */
#define MASKO_PANEL_X_GAP          6
#define MASKO_PANEL_Y_GAP          0

/* ---------------------------------------------------------- decode ----- */

/**
 * Measured esp_jpeg baseline decode throughput: 7.4 Mpx/s at 466x466 q=9 on
 * one core (29,403 us for 217,156 px). The SDK_PLAN estimated 3.2-4.1 Mpx/s
 * from third-party GIF benchmarks, which was ~2x pessimistic.
 */
#define MASKO_DECODE_MPX_S         7.4f

/* ------------------------------------------------------------ push ----- */

/**
 * Measured effective QSPI throughput: 26,542 us for 434,312 B = 16.4 MB/s.
 * The theoretical 40 MHz quad figure is 20 MB/s; the ~18% loss is protocol
 * overhead. At full screen the push costs MORE than the decode, which inverts
 * the plan's assumption that decode would be the binding constraint.
 */
#define MASKO_PUSH_BYTES_PER_S     16.4e6f

/* ----------------------------------------------------------- misc ------ */

/** Internal DRAM was 310,979 B free after boot. Anything larger cannot be DMA'd. */
#define MASKO_INTERNAL_DMA_BUDGET  280000u

/** esp_jpeg requires 16-byte aligned output buffers. */
#define MASKO_DECODE_BUF_ALIGN     16

/** FreeRTOS tick. 1 kHz is enough: the scheduler trims sub-millisecond waits. */
#define MASKO_TICK_HZ              1000

#endif /* MASKO_CALIB_H */