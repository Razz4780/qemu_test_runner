use crate::{
    patch_validator::{Patch, PatchValidator},
    tester::Tester,
};
use std::{io, path::PathBuf};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
};

pub struct TesterTask {
    pub tester: Tester,
    pub patch_source: UnboundedReceiver<Patch>,
}

impl TesterTask {
    pub async fn run(mut self) {
        while let Some(patch) = self.patch_source.recv().await {
            self.tester.clone().schedule(patch).await;
        }

        log::debug!("No more patches, exiting the tester task.");
    }
}

pub struct InputTask {
    pub validator: PatchValidator,
    pub patch_sink: UnboundedSender<Patch>,
}

impl InputTask {
    pub fn new(patch_sink: UnboundedSender<Patch>) -> Self {
        Self {
            patch_sink,
            validator: Default::default(),
        }
    }

    pub async fn run(mut self) -> io::Result<usize> {
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut buf = String::new();

        let mut invalid_lines = 0;

        while reader.read_line(&mut buf).await? > 0 {
            let path = PathBuf::from(&buf);
            buf.clear();

            let patch = match self.validator.validate(path.as_path()).await {
                Ok(patch) => patch,
                Err(error) => {
                    invalid_lines += 1;
                    log::warn!("Invalid path {} ignored. Error: {}.", path.display(), error);
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
