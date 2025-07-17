//! Contains structures for reporting on migration progress and results.

use std::fmt::{Display, Formatter};

/// Represents a report of a database table migration operation.
///
/// This struct captures statistics about a migration operation, including
/// the number of keys migrated and the time taken to perform the migration.
#[derive(Debug, Default)]
pub(crate) struct MigrationReport {
    /// The number of table keys that were migrated
    pub keys_count: usize,
    /// The total time taken to complete the migration
    pub elapsed_time: std::time::Duration,
}

impl Display for MigrationReport {
    /// Formats the migration report for human-readable output.
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!(
            "migrated {} keys in {} secs",
            self.keys_count,
            self.elapsed_time.as_secs_f64()
        ))
    }
}

impl MigrationReport {
    /// Creates a new migration report with the given key count and calculated elapsed time.
    ///
    /// # Arguments
    ///
    /// * `keys_count` - The number of keys that were migrated
    /// * `start_time` - The instant when the migration started (used to calculate elapsed time)
    ///
    /// # Returns
    ///
    /// A new `MigrationReport` instance with the calculated elapsed time.
    pub(crate) fn new(keys_count: usize, start_time: std::time::Instant) -> Self {
        Self { keys_count, elapsed_time: start_time.elapsed() }
    }

    /// Checks if the migration has been performed by verifying if any keys were migrated.
    ///
    /// # Returns
    ///
    /// `true` if at least one key was migrated, `false` otherwise.
    pub(crate) fn is_migrated(&self) -> bool {
        self.keys_count > 0
    }
}
