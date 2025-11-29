use std::{path::PathBuf, sync::Arc, time::Duration};

use libffmpeg::ffmpeg::ffmpeg_with_progress;
use tokio::{sync::Semaphore, task::JoinHandle};
use tokio_util::{future::FutureExt, sync::CancellationToken};
use valuable::Valuable;

use crate::{Args, ui::UiMessage};

#[derive(Debug, Valuable)]
pub struct SharedTaskContext {
    #[valuable(skip)]
    tx: tokio::sync::mpsc::Sender<UiMessage>,
    #[valuable(skip)]
    sem: Arc<Semaphore>,
    #[valuable(skip)]
    cancellation_token: CancellationToken,
}
impl SharedTaskContext {
    pub fn new(
        tx: tokio::sync::mpsc::Sender<UiMessage>,
        capacity: usize,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self {
            tx,
            sem: Arc::new(Semaphore::new(capacity)),
            cancellation_token,
        }
    }
}

#[derive(Debug, Valuable)]
pub struct Task {
    id: usize,
    input: PathBuf,
    output: PathBuf,
    #[valuable(skip)]
    args: Args,
    #[valuable(skip)]
    cx: Arc<SharedTaskContext>,
    #[valuable(skip)]
    total_duration: Duration,
}
impl Task {
    pub async fn new(
        id: usize,
        input: PathBuf,
        output: PathBuf,
        args: Args,
        cx: Arc<SharedTaskContext>,
    ) -> anyhow::Result<Self> {
        let duration = libffmpeg::duration::get_duration(
            &input.display().to_string(),
            cx.cancellation_token.child_token(),
        )
        .await?;

        Ok(Self {
            id,
            input,
            output,
            args,
            cx,
            total_duration: duration,
        })
    }

    fn spawn_monitor(
        &self,
        mut rx: tokio::sync::mpsc::Receiver<Duration>,
    ) -> (CancellationToken, JoinHandle<()>) {
        let token = self.cx.cancellation_token.child_token();

        let handle = {
            let token = token.clone();
            let tx = self.cx.tx.clone();
            let id = self.id;
            let total = self.total_duration;
            tokio::spawn(async move {
                while !token.is_cancelled() {
                    let delivery = match rx.recv().with_cancellation_token(&token).await {
                        Some(Some(delivery)) => delivery,
                        Some(None) /* closed */ => break,
                        None /* cancelled */ => break
                    };

                    let _ = tx
                        .send(UiMessage {
                            task_id: id,
                            payload: crate::ui::UiMessagePayload::Progress {
                                total,
                                current: delivery,
                            },
                        })
                        .await;
                }
            })
        };

        (token, handle)
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let _ = self
            .cx
            .tx
            .send(UiMessage::new(
                self.id,
                crate::ui::UiMessagePayload::Created {
                    input: self.input.clone(),
                    output: self.output.clone(),
                    total: self.total_duration,
                },
            ))
            .await;
        let _guard = self.cx.sem.acquire().await?;
        let _ = self
            .cx
            .tx
            .send(UiMessage::new(
                self.id,
                crate::ui::UiMessagePayload::Started,
            ))
            .await;

        let (tx, rx) = tokio::sync::mpsc::channel(100);

        let ct = self.cx.cancellation_token.child_token();
        let input = self.input.clone();
        let output = self.output.clone();
        let no_audio = self.args.no_audio;
        let no_video = self.args.no_video;
        let extra_args = self.args.ffmpeg_args.clone();

        let fut = ffmpeg_with_progress(tx, ct, move |cmd| {
            // Add input
            cmd.arg("-y");
            cmd.arg("-i").arg(&input);

            if no_audio {
                // Strip audio
                cmd.arg("-an");
            } else {
                // Copy audio
                cmd.arg("-c:a").arg("copy");
            }

            if no_video {
                // Remove video
                cmd.arg("-vn");
            } else {
                // Remux to x264
                cmd.arg("-c:v").arg("libx264");
                cmd.arg("-crf").arg("18");
                cmd.arg("-preset").arg("ultrafast");
            }

            // mov
            cmd.arg("-movflags").arg("+frag_keyframe+empty_moov");
            // mp4
            cmd.arg("-f").arg("mp4");

            if !extra_args.is_empty() {
                cmd.args(&extra_args);
            }

            cmd.arg(&output);
        });

        let (monitor_token, handle) = self.spawn_monitor(rx);

        let result = fut.await;
        monitor_token.cancel();
        handle.abort();

        match result {
            Ok(exit) => {
                let _ = self
                    .cx
                    .tx
                    .send(UiMessage::new(
                        self.id,
                        crate::ui::UiMessagePayload::Finished { exit },
                    ))
                    .await;
            }
            Err(e) => {
                let _ = self
                    .cx
                    .tx
                    .send(UiMessage::new(
                        self.id,
                        crate::ui::UiMessagePayload::Failed { error: e },
                    ))
                    .await;
            }
        }

        Ok(())
    }
}
