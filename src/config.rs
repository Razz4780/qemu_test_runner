use crate::tester::RunConfig;
use serde::Deserialize;

#[derive(Debug)]
pub enum ConfigError {}

#[derive(Deserialize)]
pub struct Config {}

impl TryFrom<Config> for RunConfig {
    type Error = ConfigError;

    fn try_from(_config: Config) -> Result<Self, Self::Error> {
        todo!()
    }
}
