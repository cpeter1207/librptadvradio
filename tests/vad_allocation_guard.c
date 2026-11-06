/** @file allocation_guard.c
 * @brief Debian/glibc allocation interposition, including calls from SWR.
 */
#include <assert.h>
#include <errno.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

extern void *__libc_malloc(size_t);
extern void *__libc_calloc(size_t, size_t);
extern void *__libc_realloc(void *, size_t);
extern void *__libc_memalign(size_t, size_t);
extern void __libc_free(void *);

static _Thread_local unsigned armed;
static _Thread_local unsigned allocations;
static _Thread_local unsigned frees;

void *malloc(size_t size)
{
	allocations += armed;
	return __libc_malloc(size);
}

void *calloc(size_t count, size_t size)
{
	allocations += armed;
	return __libc_calloc(count, size);
}

void *realloc(void *pointer, size_t size)
{
	allocations += armed;
	return __libc_realloc(pointer, size);
}

void free(void *pointer)
{
	frees += armed;
	__libc_free(pointer);
}

void *aligned_alloc(size_t alignment, size_t size)
{
	allocations += armed;
	return __libc_memalign(alignment, size);
}

int posix_memalign(void **pointer, size_t alignment, size_t size)
{
	allocations += armed;
	if (alignment < sizeof(void *) || (alignment & (alignment - 1)))
		return EINVAL;
	void *result = __libc_memalign(alignment, size);
	if (!result)
		return ENOMEM;
	*pointer = result;
	return 0;
}

void swr_test_realtime_begin(void)
{
	allocations = 0;
	frees = 0;
	armed = 1;
}

void swr_test_realtime_end(void)
{
	armed = 0;
	if (allocations || frees)
		fprintf(stderr, "callback allocation guard: %u allocations, %u frees\n",
			allocations, frees);
	assert(allocations == 0 && frees == 0);
}

void swr_test_realtime_checkpoint(const char *phase)
{
	armed = 0;
	if (allocations || frees)
		fprintf(stderr, "allocation occurred during %s\n", phase);
	swr_test_realtime_end();
	swr_test_realtime_begin();
}
