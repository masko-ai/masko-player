/*
 * Rust's standard library refers to realpath(), which ESP-IDF's C library does
 * not have. The engine never calls it; this answers "not supported" in case
 * anything does. It is weak, so a real one, if ESP-IDF adds it, wins.
 */
#include <errno.h>
#include <stddef.h>

__attribute__((weak)) char *realpath(const char *path, char *resolved)
{
    (void)path;
    (void)resolved;
    errno = ENOSYS;
    return NULL;
}
