use crate::{executor::ExecutionConfig, tester};
use serde::Deserialize;
use std::{collections::HashMap, path::PathBuf};

#[derive(Deserialize)]
pub struct TestConfig {
    pub weight: u8,
    pub config: tester::Config,
}

#[derive(Deserialize)]
pub struct TestSuiteConfig {
    pub weight: u8,
    pub tests: HashMap<String, TestConfig>,
}

#[derive(Deserialize)]
pub struct Config {
    pub execution: ExecutionConfig,
    pub patch_dst: PathBuf,
    pub build: tester::Config,
    pub test_suites: HashMap<String, TestSuiteConfig>,
}
