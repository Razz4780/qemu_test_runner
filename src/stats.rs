use crate::{tester::RunReport, Error};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

#[derive(Default)]
pub struct Stats {
    builds_failed: usize,
    test_failures: HashMap<String, usize>,
    solutions: usize,
    internal_errors: HashSet<PathBuf>,
}

impl Stats {
    pub fn update(&mut self, solution_path: &Path, result: Result<&RunReport, &Error>) {
        self.solutions += 1;

        match result {
            Ok(report) => {
                if report.build().err().is_some() {
                    self.builds_failed += 1;
                }

                for (test, report) in report.tests() {
                    if report.err().is_some() {
                        *self.test_failures.entry(test.clone()).or_default() += 1;
                    }
                }
            }
            Err(_) => {
                self.internal_errors.insert(solution_path.to_path_buf());
            }
        }
    }

    pub fn builds_failed(&self) -> usize {
        self.builds_failed
    }

    pub fn test_failures(&self) -> &HashMap<String, usize> {
        &self.test_failures
    }

    pub fn solutions(&self) -> usize {
        self.solutions
    }

    pub fn internal_errors(&self) -> &HashSet<PathBuf> {
        &self.internal_errors
    }
}
