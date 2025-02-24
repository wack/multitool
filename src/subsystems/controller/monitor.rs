use crate::adapters::{BoxedMonitor, Monitor};

/// The maximum number of observations that can be recevied before we
/// recompute statistical significance.
/// If this number is too low, we'll be performing compute-intensive
/// statical tests very often. If this number is too high, we could
/// be waiting too long before computing, which could permit us to promote more eagerly.
const DEFAULT_MAX_BATCH_SIZE: usize = 512;

pub struct MonitorController {
    // monitor: BoxedMonitor,
}

/*
impl MonitorController {
    pub fn new(monitor: BoxedMonitor) -> Self {
        Self { monitor }
    }
}
*/
