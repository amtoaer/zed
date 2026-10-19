use crate::Window;
use std::{cell::Cell, rc::Rc, time::Duration};

#[derive(Clone)]
pub(crate) struct FrameRate(Rc<Cell<Option<Duration>>>);

impl FrameRate {
    pub(crate) fn new(interval: Option<Duration>) -> Self {
        Self(Rc::new(Cell::new(interval)))
    }

    pub(crate) fn interval(&self) -> Option<Duration> {
        self.0.get()
    }
}

impl Window {
    /// Sets the minimum animation frame interval while this window is inactive.
    /// Set to `None` to disable inactive-window animation frame throttling.
    pub fn set_inactive_frame_interval(&mut self, interval: Option<Duration>) {
        if self.fast_frame_rate.0.replace(interval) != interval {
            self.invalidator.wake_platform();
        }
    }
}
