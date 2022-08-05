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
            if let Err(e) = self.tester.clone().schedule(patch).await {
                eprintln!("an error occurred: {}", e);
            }
        }
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

    pub async fn run(mut self) -> io::Result<()> {
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut buf = String::new();

        while reader.read_line(&mut buf).await? > 0 {
            let path = PathBuf::from(&buf);
            buf.clear();

            let patch = match self.validator.validate(path.as_path()).await {
                Ok(patch) => patch,
                Err(error) => {
                    eprintln!("Invalid path {}: {}", path.display(), error);
                    continue;
                }
            };

            if self.patch_sink.send(patch).is_err() {
                break;
            }
        }

        Ok(())
    }
}
