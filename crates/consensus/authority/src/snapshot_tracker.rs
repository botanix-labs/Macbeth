#[derive(Debug)]
pub(crate) struct ParallelSnapshots {
    // Historical tracking
    historical_snapshot_id: u64,
    historical_chunk_id: u64,
    historical_size: usize,
    historical_chunk_size: usize,
    historical_range: Option<(u64, u64)>,
    last_historical_block: Option<u64>,

    // Live tracking
    live_snapshot_id: u64,
    live_chunk_id: u64,
    live_size: usize,
    live_chunk_size: usize,
    last_live_block: Option<u64>,

    // Common tracking
    highest_id_used: u64,
}

impl ParallelSnapshots {
    pub(crate) fn new(
        initial_id: u64,
        initial_size: usize,
        historical_range: Option<(u64, u64)>,
    ) -> Self {
        Self {
            historical_snapshot_id: initial_id,
            historical_chunk_id: 0,
            historical_size: if historical_range.is_some() { initial_size } else { 0 }, /* Only use initial size for historical if we're doing historical sync */
            historical_chunk_size: 0,
            historical_range,
            last_historical_block: None,

            live_snapshot_id: initial_id + 1,
            live_chunk_id: 0,
            live_size: if historical_range.is_none() { initial_size } else { 0 }, /* Use initial size for live if we're not doing historical sync */
            live_chunk_size: 0,
            last_live_block: None,

            highest_id_used: initial_id + 1,
        }
    }

    pub(crate) fn current_snapshot_id(&self, is_historical: bool) -> u64 {
        if is_historical {
            self.historical_snapshot_id
        } else {
            self.live_snapshot_id
        }
    }

    pub(crate) fn current_chunk_id(&self, is_historical: bool) -> u64 {
        if is_historical {
            self.historical_chunk_id
        } else {
            self.live_chunk_id
        }
    }

    pub(crate) fn current_size(&self, is_historical: bool) -> usize {
        if is_historical {
            self.historical_size
        } else {
            self.live_size
        }
    }

    pub(crate) fn current_chunk_size(&self, is_historical: bool) -> usize {
        if is_historical {
            self.historical_chunk_size
        } else {
            self.live_chunk_size
        }
    }

    pub(crate) fn add_size(&mut self, block_size: usize, is_historical: bool) {
        if is_historical {
            self.historical_size += block_size;
            self.historical_chunk_size += block_size;
        } else {
            self.live_size += block_size;
            self.live_chunk_size += block_size;
        }
    }

    pub(crate) fn reset_snapshot_size(&mut self, is_historical: bool) {
        if is_historical {
            self.historical_size = 0;
            self.historical_chunk_size = 0;
            self.historical_chunk_id = 0;
        } else {
            self.live_size = 0;
            self.live_chunk_size = 0;
            self.live_chunk_id = 0;
        }
    }

    pub(crate) fn reset_chunk_size(&mut self, is_historical: bool) {
        if is_historical {
            self.historical_chunk_size = 0;
            self.historical_chunk_id += 1;
        } else {
            self.live_chunk_size = 0;
            self.live_chunk_id += 1;
        }
    }

    pub(crate) fn increment_snapshot_id(&mut self, is_historical: bool) -> u64 {
        if is_historical {
            self.historical_snapshot_id = self.highest_id_used + 1;
            self.highest_id_used = self.historical_snapshot_id;
            self.historical_snapshot_id
        } else {
            self.live_snapshot_id = self.highest_id_used + 1;
            self.highest_id_used = self.live_snapshot_id;
            self.live_snapshot_id
        }
    }

    pub(crate) fn is_historical_block(&self, block_number: u64) -> bool {
        self.historical_range
            .map(|(start, end)| block_number >= start && block_number <= end)
            .unwrap_or(false)
    }

    pub(crate) fn update_last_block(&mut self, block_number: u64, is_historical: bool) {
        if is_historical {
            self.last_historical_block = Some(block_number);
        } else {
            self.last_live_block = Some(block_number);
        }
    }

    pub(crate) fn validate_block_sequence(&self, block_number: u64, is_historical: bool) -> bool {
        let last_block =
            if is_historical { self.last_historical_block } else { self.last_live_block };

        last_block.map(|last| block_number > last).unwrap_or(true)
    }

    pub(crate) fn is_syncing_history_complete(&self) -> bool {
        match (self.historical_range, self.last_historical_block) {
            (Some((_, end)), Some(last)) => last >= end,
            _ => false,
        }
    }

    pub(crate) fn complete_historical_sync(&mut self) {
        // When historical sync completes, ensure live snapshots continue from the last historical
        // ID
        if self.historical_snapshot_id >= self.live_snapshot_id {
            self.live_snapshot_id = self.historical_snapshot_id + 1;
            self.highest_id_used = self.live_snapshot_id;
        }
        self.historical_range = None;
    }

    pub(crate) fn get_progress_info(&self) -> String {
        let historical_info = match self.last_historical_block {
            Some(hist) => format!(
                "Historical: snapshot {} (block {}, chunk {}), size: {:.2}MB, chunk size: {:.2}MB",
                self.historical_snapshot_id,
                hist,
                self.historical_chunk_id,
                self.historical_size as f64 / (1024.0 * 1024.0),
                self.historical_chunk_size as f64 / (1024.0 * 1024.0)
            ),
            None => "No historical progress".to_string(),
        };

        let live_info = match self.last_live_block {
            Some(live) => format!(
                "Live: snapshot {} (block {}, chunk {}), size: {:.2}MB, chunk size: {:.2}MB",
                self.live_snapshot_id,
                live,
                self.live_chunk_id,
                self.live_size as f64 / (1024.0 * 1024.0),
                self.live_chunk_size as f64 / (1024.0 * 1024.0)
            ),
            None => "No live progress".to_string(),
        };

        format!("{}, {}", historical_info, live_info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parallel_snapshot_progression() {
        let mut tracker = ParallelSnapshots::new(0, 0, Some((0, 1000)));

        // Process some historical blocks
        assert_eq!(tracker.current_snapshot_id(true), 0);
        assert_eq!(tracker.current_snapshot_id(false), 1);

        // Increment historical snapshot
        let new_hist_id = tracker.increment_snapshot_id(true);
        assert_eq!(new_hist_id, 2);

        // Increment live snapshot
        let new_live_id = tracker.increment_snapshot_id(false);
        assert_eq!(new_live_id, 3);

        // Verify highest ID tracking
        assert_eq!(tracker.highest_id_used, 3);
    }

    #[test]
    fn test_historical_sync_completion() {
        let mut tracker = ParallelSnapshots::new(0, 0, Some((0, 1000)));

        // Process historical blocks
        tracker.increment_snapshot_id(true); // ID: 2
        tracker.increment_snapshot_id(true); // ID: 3
        tracker.update_last_block(1000, true);

        // Process some live blocks
        tracker.increment_snapshot_id(false); // ID: 4

        assert!(tracker.is_syncing_history_complete());
        tracker.complete_historical_sync();

        // Verify live snapshots continue correctly
        assert_eq!(tracker.current_snapshot_id(false), 4);
    }
}
