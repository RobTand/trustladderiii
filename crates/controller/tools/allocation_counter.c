/* Test-only glibc allocation probe. Rust's controller keeps forbid(unsafe_code).
 * Explicit write markers delimit allocations across the measured test process.
 * Compile/load only on BuildBuddy; use an uninstrumented run for timings. */
#define _GNU_SOURCE
#include <stddef.h>
#include <stdint.h>
#include <stdatomic.h>
#include <stdio.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>
extern void *__libc_malloc(size_t);
extern void *__libc_calloc(size_t, size_t);
extern void *__libc_realloc(void *, size_t);
extern void __libc_free(void *);
static _Atomic int counting;
static _Atomic uint64_t allocations, bytes;
static void count(size_t n) {
    if (counting) { ++allocations; bytes += n; }
}
void *malloc(size_t n) { count(n); return __libc_malloc(n); }
void *calloc(size_t n, size_t width) { count(n * width); return __libc_calloc(n, width); }
void *realloc(void *ptr, size_t n) { count(n); return __libc_realloc(ptr, n); }
void free(void *ptr) { __libc_free(ptr); }
ssize_t write(int fd, const void *buffer, size_t n) {
    if (fd == 2 && n == 16 && memcmp(buffer, "TL3_ALLOC_START\n", 16) == 0) {
        allocations = bytes = 0; counting = 1;
    } else if (fd == 2 && n == 15 && memcmp(buffer, "TL3_ALLOC_STOP\n", 15) == 0) {
        counting = 0;
        char line[128];
        int length = snprintf(line, sizeof(line), "TL3_ALLOC_RESULT allocations=%llu bytes=%llu\n",
            (unsigned long long) allocations, (unsigned long long) bytes);
        syscall(SYS_write, 2, line, (size_t)length);
    }
    return syscall(SYS_write, fd, buffer, n);
}
