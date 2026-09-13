//! Per-test-thread allocation observation; excluded from production coverage.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    /// Counting is opt-in so concurrently running tests cannot affect a result.
    static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

struct ObservedAllocator;

fn note_allocation() {
    ALLOCATIONS.with(|count| {
        if let Some(value) = count.get() {
            count.set(Some(value + 1));
        }
    });
}

// Delegate unchanged layouts and pointers to the system allocator; observation
// uses constant-initialized TLS and does not allocate or synchronize threads.
unsafe impl GlobalAlloc for ObservedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_allocation();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_allocation();
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: ObservedAllocator = ObservedAllocator;

/// Count allocations during a bounded closure; reset even if it unwinds.
pub(crate) fn count<T>(operation: impl FnOnce() -> T) -> (T, usize) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ALLOCATIONS.with(|value| value.set(None));
        }
    }
    ALLOCATIONS.with(|value| {
        assert!(value.get().is_none(), "nested allocation measurement");
        value.set(Some(0));
    });
    let reset = Reset;
    let result = operation();
    let count = ALLOCATIONS.with(|value| value.get().expect("active measurement"));
    drop(reset);
    (result, count)
}
