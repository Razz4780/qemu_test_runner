use crate::{
    patch_validator::{Patch, PatchValidator},
    tester::Tester,
};
use std::{io, path::PathBuf};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
};

/// A task for proxying incoming solutions to the [Tester].
pub struct TesterTask {
    /// The tester which this task feeds with new solutions.
    pub tester: Tester,
    /// The channel from which this task polls inputs.
    pub patch_source: UnboundedReceiver<Patch>,
}

impl TesterTask {
    /// Runs this task until there are no more inputs.
    pub async fn run(mut self) {
        while let Some(patch) = self.patch_source.recv().await {
            self.tester.clone().schedule(patch).await;
        }

        log::debug!("No more patches, exiting the tester task.");
    }
}

/// A task for reading paths to the solutions from the stdin.
pub struct InputTask {
    /// The validator used on the solutions.
    pub validator: PatchValidator,
    /// The channel to which this task will send [Patch]es.
    pub patch_sink: UnboundedSender<Patch>,
}

impl InputTask {
    /// Runs this task until the end of stdin.
    /// Empty lines will be ignored.
    /// # Returns
    /// Number of rejected lines.
    pub async fn run(mut self) -> io::Result<usize> {
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut buf = String::new();

        let mut invalid_lines = 0;

        while reader.read_line(&mut buf).await? > 0 {
            if buf.is_empty() {
                continue;
            }

            let path = PathBuf::from(&buf);
            buf.clear();

            let patch = match self.validator.validate(path.as_path()).await {
                Ok(patch) => patch,
                Err(error) => {
                    invalid_lines += 1;
                    log::warn!(
                        "Invalid path `{}` ignored. Error: {}.",
                        path.display(),
                        error
                    );
                    continue;
                }
            };

            if self.patch_sink.send(patch).is_err() {
                log::debug!("There is no patch consumer, exiting the input task.");
                break;
            }
        }

        Ok(invalid_lines)
    }
}
