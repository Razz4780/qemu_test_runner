use crate::tester::RunReport;
use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
};

/// A struct for collecting statistics from the [RunReport]s.
#[derive(Default)]
pub struct Stats {
    builds_failed: usize,
    test_failures: HashMap<String, usize>,
    solutions: usize,
    internal_errors: HashSet<PathBuf>,
    failed_report_saves: usize,
}

impl Stats {
    /// Updates this struct with additional information.
    /// # Arguments
    /// solution_path - path to the patch file.
    /// result - test run result.
    pub fn update(&mut self, solution_path: &Path, result: &io::Result<RunReport>) {
        self.solutions += 1;

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
                self.internal_errors.insert(solution_path.to_path_buf());
            }
        }
    }

    /// Informs this struct that saving a detailed report failed.
    pub fn saving_report_failed(&mut self) {
        self.failed_report_saves += 1;
    }

    /// # Returns
    /// Number of solutions for which the building process failed.
    pub fn builds_failed(&self) -> usize {
        self.builds_failed
    }

    /// # Returns
    /// Number of solutions for which each test failed.
    pub fn test_failures(&self) -> &HashMap<String, usize> {
        &self.test_failures
    }

    /// # Returns
    /// Total number of solutions.
    pub fn solutions(&self) -> usize {
        self.solutions
    }

    /// # Returns
    /// Paths to solutions for which internal errors occurred during testing.
    pub fn internal_errors(&self) -> &HashSet<PathBuf> {
        &self.internal_errors
    }

    /// # Returns
    /// Number of reports which were not successfuly saved.
    pub fn failed_report_saves(&self) -> usize {
        self.failed_report_saves
    }
}
