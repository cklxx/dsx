use std::time::Instant;

use crate::frames::FRAME_TICK;
use crate::frames::FRAMES_WHALE;
use crate::tui::FrameRequester;

/// Drives the startup ASCII whale animation.
pub(crate) struct AsciiAnimation {
    request_frame: FrameRequester,
    start: Instant,
}

impl AsciiAnimation {
    pub(crate) fn new(request_frame: FrameRequester) -> Self {
        Self {
            request_frame,
            start: Instant::now(),
        }
    }

    pub(crate) fn schedule_next_frame(&self) {
        let tick_ms = FRAME_TICK.as_millis() as u64;
        let elapsed = self.start.elapsed().as_millis() as u64;
        let delay = tick_ms - (elapsed % tick_ms);
        self.request_frame
            .schedule_frame_in(std::time::Duration::from_millis(delay));
    }

    pub(crate) fn current_frame(&self) -> &'static str {
        let elapsed = self.start.elapsed().as_millis() as u64;
        let idx = (elapsed / FRAME_TICK.as_millis() as u64) as usize % FRAMES_WHALE.len();
        FRAMES_WHALE[idx]
    }
}
