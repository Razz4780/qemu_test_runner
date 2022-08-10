use crate::{patch_validator::Patch, tester::RunReport};
use std::{collections::HashMap, io, path::PathBuf};

/// Statistics from [Patch]es processing.
#[derive(Default)]
pub struct Stats {
    /// Number of solutions that were rejected by the [crate::patch_validator::PatchValidator].
    pub invalid_solutions: usize,
    /// Number of solutions that were accepted by the [crate::patch_validator::PatchValidator].
    pub valid_solutions: usize,
    /// Number of solutions that failed to build during the testing process.
    pub builds_failed: usize,
    /// Failures count by test.
    pub test_failures: HashMap<String, usize>,
    /// Solutions for which an internal error occurred during the testing process.
    pub internal_errors: Vec<PathBuf>,
    /// Solutions for which the report was not saved.
    pub missing_reports: Vec<PathBuf>,
}

impl Stats {
    /// # Returns
    /// Whether the whole run was successful (no errors occurred).
    pub fn success(&self) -> bool {
        self.internal_errors.is_empty() && self.missing_reports.is_empty()
    }

    /// Updates this struct with info from a finished testing process.
    /// # Arguments
    /// patch - processed solution.
    /// result - processing result.
    pub fn patch_processed(&mut self, patch: &Patch, result: &io::Result<RunReport>) {
        self.valid_solutions += 1;

        match result {
            Ok(report) => {
                if !report.build().success() {
                    self.builds_failed += 1;
                }

                for (test, report) in report.tests() {
                    if !report.success() {
                        *self.test_failures.entry(test.clone()).or_default() += 1;
                    }
                }
            }
            Err(_) => {
                self.internal_errors.push(patch.path().to_path_buf());
            }
        }
    }

    /// Updates this struct with info that saving a report failed.
    /// # Arguments
    /// patch - solution for which the report was not saved.
    pub fn saving_report_failed(&mut self, patch: &Patch) {
        self.missing_reports.push(patch.path().to_path_buf());
    }

    /// Updates this struct with info that a solution was rejected by the validator.
    pub fn solution_rejected(&mut self) {
        self.invalid_solutions += 1;
    }
}
