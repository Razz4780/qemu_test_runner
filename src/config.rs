use crate::{
    executor::ExecutorConfig,
    ssh::SshAction,
    tester::{RunConfig, Scenario, Step},
};
use serde::Deserialize;
use std::{collections::HashMap, path::PathBuf, time::Duration};

mod defaults {
    pub fn user() -> String {
        "root".into()
    }

    pub fn password() -> String {
        "password".into()
    }

    pub fn timeout_20_s() -> u64 {
        20 * 1000
    }

    pub fn poweroff_command() -> String {
        "/sbin/poweroff".into()
    }

    pub fn retries() -> usize {
        3
    }

    pub fn mode() -> i32 {
        0o777
    }

    pub fn timeout_5_s() -> u64 {
        5 * 1000
    }
}

#[derive(Debug)]
pub enum ConfigError {}

#[derive(Deserialize, PartialEq, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StepConfig {
    FileTransfer {
        from: PathBuf,
        to: PathBuf,
        mode: Option<i32>,
        timeout_ms: Option<u64>,
    },
    PatchTransfer {
        to: PathBuf,
        mode: Option<i32>,
        timeout_ms: Option<u64>,
    },
    Command {
        command: String,
        timeout_ms: Option<u64>,
    },
}

impl StepConfig {
    fn into_step(self, default_timeout: Duration, default_mode: i32) -> Step {
        match self {
            Self::FileTransfer {
                from,
                to,
                mode,
                timeout_ms,
            } => Step::Action {
                action: SshAction::Send {
                    from,
                    to,
                    mode: mode.unwrap_or(default_mode),
                },
                timeout: timeout_ms
                    .map(Duration::from_millis)
                    .unwrap_or(default_timeout),
            },
            Self::PatchTransfer {
                to,
                mode,
                timeout_ms,
            } => Step::TransferPatch {
                to,
                mode: mode.unwrap_or(default_mode),
                timeout: timeout_ms
                    .map(Duration::from_millis)
                    .unwrap_or(default_timeout),
            },
            Self::Command {
                command,
                timeout_ms,
            } => Step::Action {
                action: SshAction::Exec { cmd: command },
                timeout: timeout_ms
                    .map(Duration::from_millis)
                    .unwrap_or(default_timeout),
            },
        }
    }
}

#[derive(Deserialize)]
pub struct ScenarioConfig {
    pub retries: Option<usize>,
    pub steps: Vec<Vec<StepConfig>>,
}

impl ScenarioConfig {
    fn into_scenario(
        self,
        default_retries: usize,
        default_timeout: Duration,
        default_mode: i32,
    ) -> Scenario {
        let steps = self
            .steps
            .into_iter()
            .map(|phase_config| {
                phase_config
                    .into_iter()
                    .map(|step_config| step_config.into_step(default_timeout, default_mode))
                    .collect()
            })
            .collect();

        Scenario {
            retries: self.retries.unwrap_or(default_retries),
            steps,
        }
    }
}

#[derive(Deserialize)]
pub struct Config {
    #[serde(default = "defaults::user")]
    pub user: String,
    #[serde(default = "defaults::password")]
    pub password: String,
    #[serde(default = "defaults::timeout_20_s")]
    pub ssh_timeout_ms: u64,
    #[serde(default = "defaults::timeout_20_s")]
    pub poweroff_timeout_ms: u64,
    #[serde(default = "defaults::poweroff_command")]
    pub poweroff_command: String,
    #[serde(default = "defaults::retries")]
    pub retries: usize,
    #[serde(default = "defaults::timeout_5_s")]
    pub step_timeout_ms: u64,
    #[serde(default = "defaults::mode")]
    pub file_mode: i32,
    pub build: Option<ScenarioConfig>,
    pub tests: HashMap<String, ScenarioConfig>,
}

impl From<Config> for RunConfig {
    fn from(config: Config) -> RunConfig {
        let make_scenario = move |scenario_config: ScenarioConfig| {
            scenario_config.into_scenario(
                config.retries,
                Duration::from_millis(config.step_timeout_ms),
                config.file_mode,
            )
        };

        RunConfig {
            execution: ExecutorConfig {
                user: config.user,
                password: config.password,
                connection_timeout: Duration::from_millis(config.ssh_timeout_ms),
                poweroff_timeout: Duration::from_millis(config.poweroff_timeout_ms),
                poweroff_command: config.poweroff_command,
            },
            build: config.build.map(make_scenario).unwrap_or_default(),
            tests: config
                .tests
                .into_iter()
                .map(|(name, scenario_config)| (name, make_scenario(scenario_config)))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_deserialize() {
        let val = 0o777;
        let serialized = "0o777";
        let deserialized: i32 =
            serde_yaml::from_str(serialized).expect("failed to deserialize octal");
        assert_eq!(deserialized, val);
    }

    #[test]
    fn step_config_deserialize() {
        let val = StepConfig::FileTransfer {
            from: "./wow".into(),
            to: "./not/wow".into(),
            mode: Some(0o234),
            timeout_ms: 12.into(),
        };
        let serialized = "
        type: file_transfer
        from: ./wow
        to: ./not/wow
        mode: 0o234
        timeout_ms: 12
";
        let deserialized: StepConfig =
            serde_yaml::from_str(serialized).expect("failed to deserialize");
        assert_eq!(deserialized, val);
    }

    #[test]
    fn defaults_propagation() {
        let config = Config {
            user: "".into(),
            password: "".into(),
            ssh_timeout_ms: 1,
            poweroff_timeout_ms: 0,
            poweroff_command: "".into(),
            retries: 1,
            step_timeout_ms: 1,
            file_mode: 0o777,
            build: Some(ScenarioConfig {
                retries: None,
                steps: vec![vec![StepConfig::PatchTransfer {
                    to: "./wow".into(),
                    mode: None,
                    timeout_ms: None,
                }]],
            }),
            tests: Default::default(),
        };

        let run_config = RunConfig::from(config);

        assert_eq!(run_config.build.retries, 1);
        match &run_config.build.steps[0][0] {
            Step::TransferPatch { to, mode, timeout } => {
                assert_eq!(to, &PathBuf::from("./wow"));
                assert_eq!(*mode, 0o777);
                assert_eq!(timeout.as_millis(), 1);
            }
            other => panic!("unexpected enum option: {:?}", other),
        }
    }
}
