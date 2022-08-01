use crate::{executor::ExecutionConfig, tester};
use serde::Deserialize;
use std::{collections::HashMap, path::PathBuf};

#[derive(Deserialize)]
pub struct Config {
    pub execution: ExecutionConfig,
    pub patch_dst: PathBuf,
    pub build: tester::Config,
    pub tests: HashMap<String, tester::Config>,
}
