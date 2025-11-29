use std::io::BufRead;
use std::sync::Arc;
use std::{path::PathBuf, time::Duration};

use anyhow::bail;
use clap::{Parser, ValueEnum};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, info_span};
use valuable::Valuable;

use crate::tasks::{SharedTaskContext, Task};
use crate::ui::ui_spawn;

mod log;
mod path;
mod tasks;
mod ui;

#[derive(ValueEnum, Debug, Clone, Copy, Valuable)]
pub enum OutputFormat {
    Verbose,
    Human,
    Json,
    JsonPretty,
}

#[derive(Parser, Debug, Valuable, Clone)]
pub struct Args {
    // The input path, many paths are supported by passing `-i -`, and passing the list into stdin
    #[arg(short, long)]
    input: String,

    #[arg(short, long, default_value = "{SLUG}.renc.mp4")]
    output: String,

    #[arg(long)]
    no_audio: bool,

    #[arg(long)]
    no_video: bool,

    #[arg(short = 'y', long = "overwrite")]
    overwrite_output: bool,

    #[arg(short, long, value_enum, default_value_t=OutputFormat::Human)]
    format: OutputFormat,

    #[allow(clippy::struct_field_names)]
    #[arg(last = true, allow_hyphen_values = true)]
    ffmpeg_args: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    log::register_tracing_subscriber(!matches!(&args.format, OutputFormat::Verbose));

    let span = info_span!("ffrenc::main").entered();

    let cancellation_token = CancellationToken::new();
    libsignal::cancel_after_signal(cancellation_token.clone());

    let mut inputs = vec![];

    if &args.input == "-" {
        let stdin = std::io::stdin();
        let stdin = stdin.lock();
        let mut stdin = stdin.lines();

        while let Some(line) = stdin.next().transpose()? {
            if line.trim().is_empty() {
                continue;
            }

            match PathBuf::from(&line).canonicalize() {
                Ok(path) => inputs.push(path),
                Err(e) => {
                    tracing::warn!("Unable to canonicalize input path {line}: {e}");
                }
            }
        }
    } else {
        inputs.push(PathBuf::from(&args.input).canonicalize()?);
    }

    if inputs.is_empty() {
        anyhow::bail!("No inputs specified");
    }

    tracing::info!(
        inputs = inputs.as_value(),
        "Validated {} inputs",
        inputs.len()
    );

    let task_specs = inputs
        .into_iter()
        .map(|input| {
            let slug = input
                .file_stem()
                .expect("Failed to find file stem")
                .to_str()
                .expect("Failed to convert file stem to string")
                .to_string();

            let output = std::env::current_dir()?.join(args.output.replace("{SLUG}", &slug));

            if !args.overwrite_output && output.exists() {
                bail!(
                    "Output file (\"{}\") already exists (-y/--overwrite to overwrite)",
                    output.display()
                )
            }

            Ok((input, output))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let (tx, rx) = tokio::sync::mpsc::channel(100);
    let mut tasks = JoinSet::new();

    let cx = Arc::new(SharedTaskContext::new(
        tx,
        1,
        cancellation_token.child_token(),
    ));

    for (id, (input, output)) in task_specs.into_iter().enumerate() {
        let task = Task::new(id, input, output, args.clone(), cx.clone()).await?;
        tracing::debug!(task = task.as_value(), "Enqueued task");
        tasks.spawn(task.run().instrument(span.clone()));
    }

    let (ui_token, ui_handle) = ui_spawn(
        rx,
        cancellation_token.child_token(),
        args.format,
        span.clone(),
    );

    tasks.join_all().await;
    ui_token.cancel();

    tokio::time::timeout(Duration::from_secs(1), ui_handle)
        .await
        .expect("Timed out waiting for UI to exit")
        .expect("Timed out waiting for UI to exit 2?")
        .expect("UI Exited unsuccessfully");

    Ok(())
}
