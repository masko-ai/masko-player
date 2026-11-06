/**
 * @file masko_player.c
 * @brief Two-task playback: decode on core 1, push on core 0, double buffered.
 */
#include "masko_player.h"

#include <string.h>
#include <stdlib.h>

#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "freertos/semphr.h"
#include "freertos/queue.h"

#include "esp_log.h"
#include "esp_timer.h"
#include "esp_rom_sys.h"
#include "esp_heap_caps.h"
#include "masko_codec.h"
#include "masko_calib.h"

static const char *TAG = "masko_player";

#define DECODE_TASK_STACK 4096
#define DECODE_TASK_PRIO  6
#define PUSH_TASK_STACK   4096
#define PUSH_TASK_PRIO    7

typedef struct { int buf; uint16_t x, y; } frame_t;

typedef struct {
    uint8_t         *buf[3];      /* PSRAM; third buffer holds a prepared clip */
    uint16_t         w, h;

    QueueHandle_t    ready;       /* completed frame descriptors */
    QueueHandle_t    free_bufs;   /* indices whose buffers are safe to decode into */
    SemaphoreHandle_t tasks_done; /* both tasks signal before stop returns */

    volatile bool    running;
    int8_t           decode_core;
    masko_decoder_t *decoder;
} swap_t;

struct masko_player_t {
    masko_pack_t     pack;
    masko_display_t *display;
    swap_t           swap;

    /* --- clip switching -------------------------------------------------- *
     * A queued pack waits here until the decode task reaches a frame
     * boundary and takes it over. Keeping the hand-off inside the decode task
     * is what keeps the switch seamless: no task is stopped, no frame buffer
     * is freed mid-flight, and the frame rate is re-read from the new pack. */
    SemaphoreHandle_t pack_lock;      /* guards the fields below */
    masko_pack_t      next_pack;
    bool              has_next;
    bool              next_reverse;
    bool              next_immediate;
    volatile bool     reverse;
    volatile bool     prepared_busy;  /* reserved until its frame is presented */
    masko_decoder_t  *prepare_decoder; /* independent decoder for clip preflight */

    volatile uint32_t frame_idx;      /* shared with the decode task */
    volatile bool     clip_ended;     /* non-looping pack holds its last frame */
    volatile int64_t  period_us;      /* frame period of the active pack */
    volatile uint16_t bbox_x;         /* origin of the active pack's bbox */
    volatile uint16_t bbox_y;
    volatile uint8_t  max_fps;
    volatile float    active_fps;

    masko_event_cb_t on_event;
    void            *user;
    bool             stats_enabled;

    TaskHandle_t decode_task;
    TaskHandle_t push_task;

    volatile uint32_t frames_shown;
    volatile uint32_t frames_dropped;
    volatile uint32_t underruns;
    volatile int64_t  decode_us_max;
    volatile int64_t  push_us_max;
    volatile int64_t  decode_us_total;
    volatile int64_t  push_us_total;
    volatile int64_t  first_frame_us;
};

/* --------------------------------------------------------------- helpers */

static void sleep_until(int64_t delay_us)
{
    if (delay_us <= 0) {
        return;
    }
    const int64_t start = esp_timer_get_time();
    if (delay_us > 2000) {
        vTaskDelay(pdMS_TO_TICKS((delay_us - 1000) / 1000));
    }
    const int64_t elapsed = esp_timer_get_time() - start;
    if (elapsed < delay_us) {
        esp_rom_delay_us((uint32_t)(delay_us - elapsed));
    }
}

/* --------------------------------------------------------- decode task --- */
/* Runs on core 1. CPU-bound in esp_jpeg; never touches the panel. */

static void decode_task(void *arg)
{
    masko_player_t *p = (masko_player_t *)arg;
    swap_t *sw = &p->swap;

    const size_t need = masko_frame_bytes(sw->w, sw->h);

    int64_t next_due = esp_timer_get_time() + p->period_us;

    while (sw->running) {
        /* Snapshot a candidate, but keep the active pack until its first frame
         * decodes successfully. This preserves the old picture on bad media. */
        masko_pack_t frame_pack = p->pack;
        bool frame_reverse = p->reverse;
        bool switching = false;
        const bool boundary = p->clip_ended || p->frame_idx == 0;
        if (xSemaphoreTake(p->pack_lock, pdMS_TO_TICKS(0)) == pdTRUE) {
            if (p->has_next && (boundary || p->next_immediate)) {
                frame_pack = p->next_pack;
                frame_reverse = p->next_reverse;
                p->has_next = false;
                p->next_immediate = false;
                switching = true;
            }
            xSemaphoreGive(p->pack_lock);
        }

        /* A one-shot clip leaves its final frame visible until the controller
         * queues the next clip. This avoids wrapping to frame zero while an
         * authored transition or target loop is being prepared. */
        if (p->clip_ended && !switching) {
            vTaskDelay(pdMS_TO_TICKS(2));
            continue;
        }

        const uint8_t *jpg = NULL;
        uint32_t jlen = 0;
        const uint32_t frame_idx = switching ? 0 : p->frame_idx;
        if (!switching && masko_pack_frame(&frame_pack, frame_idx, &jpg, &jlen) != MASKO_OK) {
            p->frames_dropped++;
            if (!switching)
                p->frame_idx = (p->frame_idx + 1) % p->pack.hdr.frame_count;
            else if (p->on_event)
                p->on_event(MASKO_EV_ERROR, p->user);
            continue;
        }

        /* Acquire a specific free buffer. A generic semaphore can release the
         * wrong buffer and let decode overwrite pixels still being pushed. */
        int buf = switching ? 2 : -1;
        if (!switching && xQueueReceive(sw->free_bufs, &buf, pdMS_TO_TICKS(500)) != pdTRUE) {
            if (!sw->running) break;
            continue;
        }
        if (!sw->running) break;

        const int64_t t0 = esp_timer_get_time();
        const uint8_t *out = NULL;
        uint16_t w = 0, h = 0;
        const esp_err_t err = switching ? ESP_OK :
            masko_decoder_decode(sw->decoder, jpg, jlen, sw->buf[buf], need,
                                 &out, &w, &h);
        const int64_t dt = esp_timer_get_time() - t0;

        if (err != ESP_OK) {
            p->frames_dropped++;
            if (p->on_event) p->on_event(MASKO_EV_ERROR, p->user);
            if (sw->running) xQueueSend(sw->free_bufs, &buf, 0);
        } else {
            if (switching) {
                out = sw->buf[buf]; /* already decoded and validated by set_pack */
                p->pack = frame_pack;
                p->reverse = frame_reverse;
                p->frame_idx = frame_reverse ? frame_pack.hdr.frame_count - 1 : 0;
                p->clip_ended = false;
                p->bbox_x = frame_pack.hdr.bbox_x;
                p->bbox_y = frame_pack.hdr.bbox_y;
                const float fps = masko_pack_fps(&frame_pack);
                p->period_us = fps > 0.0f ? (int64_t)(1e6f / fps) : 41666;
                if (p->max_fps && p->period_us < 1000000LL / p->max_fps)
                    p->period_us = 1000000LL / p->max_fps;
                p->active_fps = fps;
                next_due = esp_timer_get_time();
                ESP_LOGI(TAG, "switched clip: %u frames, %.1f fps",
                         p->pack.hdr.frame_count, (double)fps);
                if (p->on_event) p->on_event(MASKO_EV_CLIP_START, p->user);
            }
            if (dt > p->decode_us_max) p->decode_us_max = dt;
            p->decode_us_total += dt;
            if (p->first_frame_us == 0) p->first_frame_us = esp_timer_get_time();
            frame_t frame = { .buf = buf,
                              .x = switching ? frame_pack.hdr.bbox_x : p->bbox_x,
                              .y = switching ? frame_pack.hdr.bbox_y : p->bbox_y };
            if (xQueueSend(sw->ready, &frame, pdMS_TO_TICKS(500)) != pdTRUE) {
                p->frames_dropped++;
                if (sw->running) xQueueSend(sw->free_bufs, &buf, 0);
            }
        }

        if (err == ESP_OK || !switching) {
            const bool at_end = p->reverse ? p->frame_idx == 0 :
                                p->frame_idx + 1 >= p->pack.hdr.frame_count;
            if (at_end) {
                if (p->pack.hdr.flags & MASKO_FLAG_LOOP) {
                    p->frame_idx = p->reverse ? p->pack.hdr.frame_count - 1 : 0;
                } else {
                    p->clip_ended = true;
                }
                if (p->on_event) p->on_event(MASKO_EV_CLIP_END, p->user);
            } else {
                p->frame_idx = p->reverse ? p->frame_idx - 1 : p->frame_idx + 1;
            }
        }

        /* Pace against an absolute schedule so rounding cannot accumulate drift. */
        next_due += p->period_us;
        const int64_t now = esp_timer_get_time();
        if (next_due > now) {
            sleep_until(next_due - now);
        } else {
            next_due = now;   /* behind: resync instead of bursting */
        }
    }

    sw->running = false;
    xSemaphoreGive(sw->tasks_done);
    vTaskDelete(NULL);
}

/* ----------------------------------------------------------- push task --- */
/* Runs on core 0. Blocks in the SPI DMA engine, leaving the CPU free for the
 * decoder on the other core. This is where the overlap comes from. */

static void push_task(void *arg)
{
    masko_player_t *p = (masko_player_t *)arg;
    swap_t *sw = &p->swap;

    uint32_t shown = 0;

    while (sw->running) {
        frame_t frame;
        if (xQueueReceive(sw->ready, &frame, pdMS_TO_TICKS(200)) != pdTRUE) {
            if (!sw->running) break;
            p->underruns++;
            if (p->on_event) p->on_event(MASKO_EV_UNDERRUN, p->user);
            continue;
        }
        if (!sw->running) break;

        const int64_t t0 = esp_timer_get_time();
        const esp_err_t err = masko_display_blit(p->display,
                                                  frame.x, frame.y,
                                                  sw->w, sw->h, sw->buf[frame.buf]);
        const int64_t dt = esp_timer_get_time() - t0;

        if (err == ESP_OK) {
            if (dt > p->push_us_max) p->push_us_max = dt;
            p->push_us_total += dt;
            shown++;
            p->frames_shown = shown;
        } else {
            p->frames_dropped++;
            if (p->on_event) p->on_event(MASKO_EV_ERROR, p->user);
        }

        if (sw->running) {
            xQueueSend(sw->free_bufs, &frame.buf, 0);
            if (frame.buf == 2) p->prepared_busy = false;
        }
    }

    sw->running = false;
    xSemaphoreGive(sw->tasks_done);
    vTaskDelete(NULL);
}

/* ------------------------------------------------------------ lifecycle - */

esp_err_t masko_player_create(const masko_player_cfg_t *cfg, masko_player_t **out)
{
    if (cfg == NULL || out == NULL || cfg->display == NULL || cfg->pack_data == NULL) {
        return ESP_ERR_INVALID_ARG;
    }

    masko_player_t *p = calloc(1, sizeof(*p));
    if (p == NULL) {
        return ESP_ERR_NO_MEM;
    }

    const int rc = masko_pack_open(cfg->pack_data, cfg->pack_size, &p->pack);
    if (rc != MASKO_OK) {
        ESP_LOGE(TAG, "pack rejected: %s", masko_strerror(rc));
        free(p);
        return ESP_ERR_INVALID_VERSION;
    }

    if ((uint32_t)p->pack.hdr.bbox_x + p->pack.hdr.bbox_w >
            (uint32_t)cfg->display->cfg.canvas_w ||
        (uint32_t)p->pack.hdr.bbox_y + p->pack.hdr.bbox_h >
            (uint32_t)cfg->display->cfg.canvas_h) {
        free(p);
        return ESP_ERR_INVALID_SIZE;
    }

    p->display = cfg->display;
    p->on_event = cfg->on_event;
    p->user = cfg->user;
    p->stats_enabled = cfg->stats_enabled;

    swap_t *sw = &p->swap;
    sw->w = p->pack.hdr.bbox_w;
    sw->h = p->pack.hdr.bbox_h;
    sw->decode_core = cfg->decode_core;

    const size_t need = masko_frame_bytes(sw->w, sw->h);
    for (int i = 0; i < 3; i++) {
        sw->buf[i] = heap_caps_aligned_alloc(MASKO_DECODE_BUF_ALIGN, need,
                                             MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT);
        if (sw->buf[i] == NULL) {
            ESP_LOGE(TAG, "cannot allocate decode buffer %d (%u B)", i, (unsigned)need);
            for (int j = 0; j < i; j++) heap_caps_free(sw->buf[j]);
            free(p);
            return ESP_ERR_NO_MEM;
        }
    }

    esp_err_t err = masko_decoder_new(sw->w, sw->h, &sw->decoder);
    if (err != ESP_OK) {
        for (int i = 0; i < 3; i++) heap_caps_free(sw->buf[i]);
        free(p);
        return err;
    }

    err = masko_decoder_new(sw->w, sw->h, &p->prepare_decoder);
    if (err != ESP_OK) {
        masko_decoder_free(sw->decoder);
        for (int i = 0; i < 3; i++) heap_caps_free(sw->buf[i]);
        free(p);
        return err;
    }

    sw->ready = xQueueCreate(2, sizeof(frame_t));
    sw->free_bufs = xQueueCreate(3, sizeof(int));
    sw->tasks_done = xSemaphoreCreateCounting(2, 0);
    p->pack_lock = xSemaphoreCreateMutex();
    if (sw->ready == NULL || sw->free_bufs == NULL || sw->tasks_done == NULL ||
        p->pack_lock == NULL) {
        ESP_LOGE(TAG, "cannot create sync primitives");
        if (sw->ready) vQueueDelete(sw->ready);
        if (sw->free_bufs) vQueueDelete(sw->free_bufs);
        if (sw->tasks_done) vSemaphoreDelete(sw->tasks_done);
        if (p->pack_lock) vSemaphoreDelete(p->pack_lock);
        masko_decoder_free(p->prepare_decoder);
        masko_decoder_free(sw->decoder);
        for (int i = 0; i < 3; i++) heap_caps_free(sw->buf[i]);
        free(p);
        return ESP_ERR_NO_MEM;
    }
    /* Each buffer has an independent ownership token. */
    const int buf0 = 0, buf1 = 1;
    xQueueSend(sw->free_bufs, &buf0, 0);
    xQueueSend(sw->free_bufs, &buf1, 0);

    /* Shared playback state, updated by the decode task on a clip switch. */
    const float fps0 = masko_pack_fps(&p->pack);
    p->period_us = fps0 > 0.0f ? (int64_t)(1e6f / fps0) : 41666;
    p->active_fps = fps0;
    p->max_fps = cfg->max_fps;
    if (p->max_fps && p->period_us < 1000000LL / p->max_fps)
        p->period_us = 1000000LL / p->max_fps;
    p->frame_idx = 0;
    p->clip_ended = false;
    p->bbox_x    = p->pack.hdr.bbox_x;
    p->bbox_y    = p->pack.hdr.bbox_y;
    p->has_next  = false;
    p->next_pack = p->pack;   /* keep a valid copy; never used until has_next */

    ESP_LOGI(TAG, "pack: %u frames, bbox %ux%u+%u+%u, %.1f fps, %.1f ms decode (measured)",
             p->pack.hdr.frame_count, sw->w, sw->h, p->pack.hdr.bbox_x, p->pack.hdr.bbox_y,
             (double)masko_pack_fps(&p->pack),
             (double)sw->w * sw->h / (1e6 * MASKO_DECODE_MPX_S));

    *out = p;
    return ESP_OK;
}

static esp_err_t set_pack_direction(masko_player_t *p, const uint8_t *data,
                                    size_t size, bool reverse, bool immediate)
{
    if (p == NULL || data == NULL || size == 0) {
        return ESP_ERR_INVALID_ARG;
    }

    /* Validate before queueing, so a bad pack is rejected at the call site
     * rather than half-way through a frame. */
    masko_pack_t np;
    const int rc = masko_pack_open(data, size, &np);
    if (rc != MASKO_OK) {
        ESP_LOGE(TAG, "pack rejected: %s", masko_strerror(rc));
        return ESP_ERR_INVALID_VERSION;
    }

    /* The decode buffers and the panel update rectangle are sized from the
     * geometry of the pack the player was created with. A clip of a different
     * size would overrun them, so refuse it rather than corrupt memory.
     * Bundle clips with a shared bbox and this never triggers. */
    if (np.hdr.bbox_w != p->swap.w || np.hdr.bbox_h != p->swap.h) {
        ESP_LOGE(TAG, "clip geometry %ux%u does not match the player's %ux%u",
                 np.hdr.bbox_w, np.hdr.bbox_h, p->swap.w, p->swap.h);
        return ESP_ERR_INVALID_SIZE;
    }
    if ((uint32_t)np.hdr.bbox_x + np.hdr.bbox_w >
            (uint32_t)p->display->cfg.canvas_w ||
        (uint32_t)np.hdr.bbox_y + np.hdr.bbox_h >
            (uint32_t)p->display->cfg.canvas_h) {
        return ESP_ERR_INVALID_SIZE;
    }

    xSemaphoreTake(p->pack_lock, portMAX_DELAY);
    if (p->has_next || p->prepared_busy) {
        xSemaphoreGive(p->pack_lock);
        return ESP_ERR_INVALID_STATE;
    }

    /* Pre-decode the candidate's first frame into its reserved PSRAM buffer.
     * The active decoder and visible clip keep running while this independent
     * decoder verifies that the handoff frame is actually usable. */
    const uint8_t *jpg = NULL;
    uint32_t jlen = 0;
    const uint32_t first = reverse ? np.hdr.frame_count - 1 : 0;
    const int frc = masko_pack_frame(&np, first, &jpg, &jlen);
    const size_t need = masko_frame_bytes(p->swap.w, p->swap.h);
    const esp_err_t prep_err = frc == MASKO_OK ?
        masko_decoder_decode(p->prepare_decoder, jpg, jlen, p->swap.buf[2],
                             need, NULL, NULL, NULL) : ESP_FAIL;
    if (prep_err != ESP_OK) {
        xSemaphoreGive(p->pack_lock);
        if (p->on_event) p->on_event(MASKO_EV_ERROR, p->user);
        return prep_err;
    }
    p->next_pack = np;
    p->next_reverse = reverse;
    p->next_immediate = immediate;
    p->has_next  = true;
    p->prepared_busy = true;
    xSemaphoreGive(p->pack_lock);

    ESP_LOGI(TAG, "clip queued%s: %u frames, %.1f fps",
             immediate ? " for immediate handoff" : "",
             np.hdr.frame_count, (double)masko_pack_fps(&np));
    return ESP_OK;
}

esp_err_t masko_player_set_pack_direction(masko_player_t *p, const uint8_t *data,
                                          size_t size, bool reverse)
{
    return set_pack_direction(p, data, size, reverse, false);
}

esp_err_t masko_player_set_pack_direction_immediate(masko_player_t *p,
                                                    const uint8_t *data,
                                                    size_t size, bool reverse)
{
    return set_pack_direction(p, data, size, reverse, true);
}

esp_err_t masko_player_set_pack(masko_player_t *p, const uint8_t *data, size_t size)
{
    return masko_player_set_pack_direction(p, data, size, false);
}

bool masko_player_has_pending_pack(const masko_player_t *p)
{
    if (p == NULL || p->pack_lock == NULL) return false;
    if (xSemaphoreTake(p->pack_lock, pdMS_TO_TICKS(0)) != pdTRUE) return true;
    const bool pending = p->has_next;
    xSemaphoreGive(p->pack_lock);
    return pending;
}

esp_err_t masko_player_play(masko_player_t *p)
{
    if (p == NULL || p->decode_task != NULL) {
        return ESP_ERR_INVALID_STATE;
    }

    p->swap.running = true;
    p->frames_shown = 0;
    p->frames_dropped = 0;
    p->underruns = 0;
    p->decode_us_max = p->push_us_max = 0;
    p->decode_us_total = p->push_us_total = 0;
    p->first_frame_us = 0;
    xQueueReset(p->swap.ready);
    xQueueReset(p->swap.free_bufs);
    const int buf0 = 0, buf1 = 1;
    xQueueSend(p->swap.free_bufs, &buf0, 0);
    xQueueSend(p->swap.free_bufs, &buf1, 0);

    BaseType_t ok = xTaskCreatePinnedToCore(push_task, "masko_push", PUSH_TASK_STACK,
                                            p, PUSH_TASK_PRIO, &p->push_task, 0);
    if (ok != pdPASS) {
        p->swap.running = false;
        frame_t wake_frame = { .buf = 0, .x = 0, .y = 0 };
        xQueueSend(p->swap.ready, &wake_frame, 0);
        xSemaphoreTake(p->swap.tasks_done, portMAX_DELAY);
        p->push_task = NULL;
        return ESP_ERR_NO_MEM;
    }
    if (p->swap.decode_core < 0) {
        ok = xTaskCreate(decode_task, "masko_dec", DECODE_TASK_STACK, p,
                         DECODE_TASK_PRIO, &p->decode_task);
    } else {
        ok = xTaskCreatePinnedToCore(decode_task, "masko_dec", DECODE_TASK_STACK, p,
                                     DECODE_TASK_PRIO, &p->decode_task,
                                     (BaseType_t)p->swap.decode_core);
    }
    if (ok != pdPASS) {
        p->swap.running = false;
        return ESP_ERR_NO_MEM;
    }
    return ESP_OK;
}

void masko_player_stop(masko_player_t *p)
{
    if (p == NULL) {
        return;
    }
    if (p->decode_task == NULL && p->push_task == NULL) return;
    p->swap.running = false;
    /* Wake both tasks if one is waiting on a queue. They each acknowledge
     * exit, so buffers and synchronization objects are never freed early. */
    int wake = 0;
    xQueueSend(p->swap.free_bufs, &wake, 0);
    frame_t frame = { .buf = 0, .x = 0, .y = 0 };
    xQueueSend(p->swap.ready, &frame, 0);
    const int task_count = (p->decode_task != NULL) + (p->push_task != NULL);
    for (int i = 0; i < task_count; i++)
        xSemaphoreTake(p->swap.tasks_done, portMAX_DELAY);
    p->decode_task = NULL;
    p->push_task = NULL;
}

void masko_player_destroy(masko_player_t *p)
{
    if (p == NULL) {
        return;
    }
    masko_player_stop(p);
    if (p->swap.ready)  vQueueDelete(p->swap.ready);
    if (p->swap.free_bufs) vQueueDelete(p->swap.free_bufs);
    if (p->swap.tasks_done) vSemaphoreDelete(p->swap.tasks_done);
    if (p->pack_lock)   vSemaphoreDelete(p->pack_lock);
    if (p->prepare_decoder) masko_decoder_free(p->prepare_decoder);
    if (p->swap.decoder) masko_decoder_free(p->swap.decoder);
    for (int i = 0; i < 3; i++)
        if (p->swap.buf[i]) heap_caps_free(p->swap.buf[i]);
    free(p);
}

masko_stats_t masko_player_stats(const masko_player_t *p)
{
    masko_stats_t s;
    memset(&s, 0, sizeof(s));
    if (p == NULL) {
        return s;
    }
    s.frames_shown    = p->frames_shown;
    s.frames_dropped  = p->frames_dropped;
    s.underruns       = p->underruns;
    s.decode_us_max   = p->decode_us_max;
    s.push_us_max     = p->push_us_max;
    if (p->frames_shown > 0) {
        s.decode_us_avg = p->decode_us_total / p->frames_shown;
        s.push_us_avg   = p->push_us_total / p->frames_shown;
        const int64_t mean = s.decode_us_avg + s.push_us_avg;
        if (mean > 0) {
            s.ceiling_fps = 1e6 / (double)mean;
        }
    }
    s.achieved_fps = p->active_fps;
    return s;
}
