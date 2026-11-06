/**
 * @file test_masko_pack.c
 * @brief Host-side tests for masko_pack.c. No ESP-IDF, no hardware.
 *
 * Run: tools/test_masko_core.sh [pack.mkpk]
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "masko_pack.h"

static int failures;
static int checks;

static void check(int cond, const char *what)
{
    checks++;
    if (!cond) {
        failures++;
    }
    printf("  [%s] %s\n", cond ? "PASS" : "FAIL", what);
}

static uint8_t *slurp(const char *path, size_t *len)
{
    FILE *f = fopen(path, "rb");
    if (!f) { fprintf(stderr, "cannot open %s\n", path); exit(2); }
    fseek(f, 0, SEEK_END); long n = ftell(f); fseek(f, 0, SEEK_SET);
    uint8_t *b = malloc((size_t)n);
    if (fread(b, 1, (size_t)n, f) != (size_t)n) { fprintf(stderr, "short read\n"); exit(2); }
    fclose(f);
    *len = (size_t)n;
    return b;
}

static void test_real_pack(const char *path)
{
    size_t size;
    uint8_t *d = slurp(path, &size);

    printf("\n%s (%zu bytes)\n", path, size);

    masko_pack_t p;
    check(masko_pack_open(d, size, &p) == MASKO_OK, "pack opens");
    check(p.hdr.frame_count > 0, "frame_count > 0");
    check(p.hdr.bbox_w % MASKO_MCU == 0, "bbox_w is MCU-aligned");
    check(masko_pack_fps(&p) > 0.0f, "fps parses");

    /* payload CRC over index_offset..EOF must match the header */
    uint32_t crc = masko_crc32(d + p.hdr.index_offset,
                               p.hdr.file_size - p.hdr.index_offset, 0);
    check(crc == p.hdr.payload_crc32, "payload CRC matches header");

    /* every frame: aligned, in range, and a complete JPEG */
    int bad_magic = 0, bad_range = 0;
    size_t total = 0, smallest = (size_t)-1, largest = 0;
    for (uint32_t i = 0; i < p.hdr.frame_count; i++) {
        const uint8_t *fd; uint32_t fl;
        if (masko_pack_frame(&p, i, &fd, &fl) != MASKO_OK) { bad_range++; continue; }
        if (fl < 4 || fd[0] != 0xFF || fd[1] != 0xD8 ||
            fd[fl - 2] != 0xFF || fd[fl - 1] != 0xD9) {
            bad_magic++;
        }
        total += fl;
        if (fl < smallest) smallest = fl;
        if (fl > largest) largest = fl;
    }
    check(bad_range == 0, "all frame offsets are in range");
    check(bad_magic == 0, "all frames are FFD8..FFD9");
    printf("  frames=%u avg=%.0fB min=%zuB max=%zuB total=%zuB\n",
           p.hdr.frame_count, (double)total / p.hdr.frame_count, smallest, largest, total);

    /* 512-byte alignment, computed from the offsets themselves */
    int misaligned = 0;
    for (uint32_t i = 0; i < p.hdr.frame_count; i++) {
        const uint8_t *fd; uint32_t fl;
        if (masko_pack_frame(&p, i, &fd, &fl) == MASKO_OK) {
            if ((size_t)(fd - d) % MASKO_PAYLOAD_ALIGN) misaligned++;
        }
    }
    check(misaligned == 0, "all frames are 512-byte aligned");

    /* out of range index must be refused */
    const uint8_t *fd; uint32_t fl;
    check(masko_pack_frame(&p, p.hdr.frame_count, &fd, &fl) == MASKO_ERR_ARG,
          "out-of-range frame index is refused");

    free(d);
}

static void test_corruption(void)
{
    printf("\nCorruption handling\n");
    size_t size;
    uint8_t *orig = slurp(PACK_PATH, &size);
    uint8_t *d = malloc(size);
    masko_pack_t p;

    memcpy(d, orig, size);
    d[0] = 'X';
    check(masko_pack_open(d, size, &p) == MASKO_ERR_MAGIC, "bad magic is rejected");

    memcpy(d, orig, size);
    d[4] = 99;
    check(masko_pack_open(d, size, &p) == MASKO_ERR_VERSION, "unknown version is rejected");

    memcpy(d, orig, size);
    d[20] ^= 0xFF;                       /* perturb bbox_w, breaks header CRC */
    check(masko_pack_open(d, size, &p) == MASKO_ERR_HEADER_CRC,
          "tampered header is caught by CRC");

    check(masko_pack_open(d, 8, &p) == MASKO_ERR_TOO_SMALL, "undersized buffer is refused");
    check(masko_pack_open(NULL, size, &p) == MASKO_ERR_ARG, "NULL is refused");

    /* truncated file: header parses, file_size check fires */
    memcpy(d, orig, size);
    check(masko_pack_open(d, size / 2, &p) == MASKO_ERR_TRUNCATED,
          "truncated file is rejected");

    free(d);
    free(orig);
}

static void test_mjpeg_indexer(void)
{
    printf("\nConcatenated MJPEG indexer\n");
    uint32_t off[64];

    check(masko_index_mjpeg((const uint8_t *)"", 0, off, 64) == 0, "empty stream -> 0");
    check(masko_index_mjpeg(NULL, 10, off, 64) == 0, "NULL -> 0");

    /* frame with SOI but no EOI must be refused */
    const uint8_t partial[] = {0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00};
    check(masko_index_mjpeg(partial, sizeof(partial), off, 64) == 0,
          "frame without EOI is rejected");

    /* two well-formed frames back to back */
    const uint8_t two[] = {
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x02, 0xFF, 0xD9,
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x02, 0xFF, 0xD9,
    };
    check(masko_index_mjpeg(two, sizeof(two), off, 64) == 2, "two adjacent frames -> 2");

    /* leading garbage between frames is skipped */
    const uint8_t padded[] = {
        0x00, 0x11, 0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x02, 0xFF, 0xD9,
        0xAB, 0xCD,
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x02, 0xFF, 0xD9,
    };
    check(masko_index_mjpeg(padded, sizeof(padded), off, 64) == 2,
          "padding between frames is skipped");

    /* capacity must be honoured so a caller's table cannot overflow */
    uint32_t small[2];
    check(masko_index_mjpeg(padded, sizeof(padded), small, 1) == 1,
          "max_frames caps the count");
}

int main(void)
{
    test_real_pack(PACK_PATH);
    test_corruption();
    test_mjpeg_indexer();
    printf("\n%s (%d checks, %d failure%s)\n",
           failures ? "FAILED" : "ALL TESTS PASSED",
           checks, failures, failures == 1 ? "" : "s");
    return failures ? 1 : 0;
}