//! Bounded native-parrot recording and playback state.
//!
//! Storage remains caller owned and preallocated.  The radio core owns only
//! the cursor and transition state so an audio callback never allocates or
//! depends on an Asterisk echo queue.

use super::RADIO_INVALID_ARGUMENT;
use std::ffi::c_int;
use std::ptr;

/// Observable state of one native-parrot recording.
#[derive(Clone, Copy, Default)]
pub(crate) struct Status {
    pub(crate) recorded_samples: u64,
    pub(crate) playback_offset: u64,
    pub(crate) playing: u32,
    pub(crate) truncated: u32,
}

/// Callback-owned cursor state bound to caller-owned canonical F32 storage.
pub(crate) struct State {
    storage: *mut f32,
    capacity: u32,
    count: u32,
    play: u32,
    playing: bool,
    truncated: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            storage: ptr::null_mut(),
            capacity: 0,
            count: 0,
            play: 0,
            playing: false,
            truncated: false,
        }
    }
}

impl State {
    /// Bind an idle recorder to stable caller-owned storage.
    pub(crate) fn bind(&mut self, storage: *mut f32, capacity: u32) -> Result<(), c_int> {
        if storage.is_null() || capacity == 0 || self.count != 0 || self.truncated {
            return Err(RADIO_INVALID_ARGUMENT);
        }
        self.storage = storage;
        self.capacity = capacity;
        self.truncated = false;
        Ok(())
    }

    /// Clear recording and playback state without releasing caller storage.
    pub(crate) fn reset(&mut self) {
        self.count = 0;
        self.play = 0;
        self.playing = false;
        self.truncated = false;
    }

    /// Apply the legacy carrier transition rules and report playback start.
    pub(crate) fn rx_transition(&mut self, was_keyed: u32, is_keyed: u32) -> bool {
        if was_keyed == 0 && is_keyed != 0 {
            self.reset();
            false
        } else if was_keyed != 0 && is_keyed == 0 && self.count != 0 {
            self.play = 0;
            self.playing = true;
            true
        } else {
            false
        }
    }

    /// Append as much of one bounded input span as the configured limit permits.
    ///
    /// # Safety
    ///
    /// `input` is readable for `frame_count` canonical F32 samples when that
    /// count is nonzero.  Bound storage remains valid for this radio object's
    /// lifetime and is accessed only by its owning callback.
    pub(crate) unsafe fn record(
        &mut self,
        input: *const f32,
        frame_count: u32,
        limit: u32,
    ) -> Result<u32, c_int> {
        if limit > self.capacity || (frame_count != 0 && input.is_null()) || self.storage.is_null()
        {
            return Err(RADIO_INVALID_ARGUMENT);
        }
        let available = limit.saturating_sub(self.count);
        let recorded = frame_count.min(available);
        if recorded != 0 {
            unsafe {
                ptr::copy_nonoverlapping(
                    input,
                    self.storage.add(self.count as usize),
                    recorded as usize,
                );
            }
        }
        self.count += recorded;
        if recorded != frame_count {
            self.truncated = true;
        }
        Ok(recorded)
    }

    /// Copy the next bounded playback span without modifying unwritten output.
    ///
    /// # Safety
    ///
    /// `output` is writable for `frame_count` canonical F32 samples whenever
    /// that count is nonzero.  Bound storage remains valid for this radio
    /// object's lifetime and is accessed only by its owning callback.
    pub(crate) unsafe fn play(&mut self, output: *mut f32, frame_count: u32) -> Result<u32, c_int> {
        if !self.playing {
            return Ok(0);
        }
        if frame_count != 0 && output.is_null() {
            return Err(RADIO_INVALID_ARGUMENT);
        }
        let copied = frame_count.min(self.count - self.play);
        if copied != 0 {
            unsafe {
                ptr::copy_nonoverlapping(
                    self.storage.add(self.play as usize),
                    output,
                    copied as usize,
                );
            }
        }
        self.play += copied;
        if self.play >= self.count {
            self.playing = false;
        }
        Ok(copied)
    }

    /// Return the current recording and playback state.
    pub(crate) fn status(&self) -> Status {
        Status {
            recorded_samples: u64::from(self.count),
            playback_offset: u64::from(self.play),
            playing: self.playing.into(),
            truncated: self.truncated.into(),
        }
    }
}
