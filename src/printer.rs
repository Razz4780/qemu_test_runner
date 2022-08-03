use crate::{tester::RunReport, Error};
use std::{
    io,
    path::{Path, PathBuf},
    pin::Pin,
};
use tokio::{
    fs::File,
    io::{AsyncWrite, AsyncWriteExt, BufWriter},
};

pub struct Printer<W> {
    reports_dir: PathBuf,
    results_target: Pin<Box<W>>,
}

impl<W> Printer<W> {
    pub fn new(reports_dir: PathBuf, results_target: W) -> Self {
        Self {
            reports_dir,
            results_target: Box::pin(results_target),
        }
    }
}

impl<W> Printer<W>
where
    W: AsyncWrite,
{
    async fn print_results(
        &mut self,
        patch: &Path,
        run_report: Result<&RunReport, &Error>,
    ) -> io::Result<()> {
        let stem = patch
            .file_stem()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "file has no stem"))?;
        let tests = match run_report {
            Ok(run_report) if run_report.build().err().is_some() => "build failed".into(),
            Ok(run_report) => {
                let failed_tests = run_report
                    .tests()
                    .iter()
                    .filter_map(|(name, report)| report.err().is_none().then_some(&name[..]))
                    .collect::<Vec<_>>();

                if failed_tests.is_empty() {
                    "OK".into()
                } else {
                    format!("tests failed: {}", failed_tests.join(","))
                }
            }
            Err(error) => format!("testing failed with error: {}", error),
        };

        let line = format!("{};{};{}\n", patch.display(), stem.to_string_lossy(), tests);
        self.results_target.write_all(line.as_bytes()).await?;

        Ok(())
    }

    async fn print_report(&self, patch: &Path, _run_report: &RunReport) -> io::Result<()> {
        let stem = patch
            .file_stem()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "file has no stem"))?;
        let file = File::create(self.reports_dir.join(stem)).await?;
        let mut buf = BufWriter::new(file);

        // TODO write report

        buf.flush().await
    }

    pub async fn print(
        &mut self,
        patch: &Path,
        run_report: Result<&RunReport, &Error>,
    ) -> io::Result<()> {
        self.print_results(patch, run_report).await?;

        if let Ok(run_report) = run_report {
            self.print_report(patch, run_report).await?;
        }

        Ok(())
    }
}
