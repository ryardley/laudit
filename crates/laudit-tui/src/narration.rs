//! Background narration for the terminal frontend.
//!
//! Narration is async and streamed. A single worker thread runs one stop at a
//! time so the redaction map is never written concurrently; navigation while a
//! stop is being narrated queues the newest stop and starts it when the current
//! one finishes. Chunks are delivered to the UI over a channel so the event loop
//! never blocks on the LLM.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use laudit_core::config::Config;
use laudit_core::llm::LlmClient;
use laudit_core::model::{CodebaseModel, Skeleton, Stop, TourChapter};
use laudit_core::narration::{self, NarrationContext, NarrationSink};

/// An event produced by the narration worker.
#[derive(Debug, Clone)]
pub enum UiEvent {
    /// A chunk of the markdown body for a stop.
    Chunk {
        /// Stop the chunk belongs to.
        stop_id: String,
        /// Markdown delta.
        delta: String,
    },
    /// A stop finished narrating and was persisted.
    StopNarrated {
        /// The narrated stop.
        stop: Stop,
        /// Warnings produced while narrating.
        warnings: Vec<String>,
    },
    /// A stop failed to narrate.
    NarrationFailed {
        /// The stop, marked as skipped.
        stop: Stop,
        /// Why narration failed.
        message: String,
    },
}

/// Everything a narration worker needs, owned so it can move to the worker.
#[derive(Debug, Clone)]
pub struct NarrationJob {
    /// Repository root.
    pub repo_root: PathBuf,
    /// Resolved engine configuration.
    pub config: Config,
    /// Recon skeleton.
    pub skeleton: Skeleton,
    /// Codebase model.
    pub model: CodebaseModel,
    /// Chapter containing the stop.
    pub chapter: TourChapter,
    /// Stop to narrate.
    pub stop: Stop,
    /// LLM client.
    pub client: LlmClient,
}

/// Owns the narration worker and the channel to the UI.
pub struct NarrationManager {
    job_tx: Sender<NarrationJob>,
    event_rx: Receiver<UiEvent>,
    active: Option<String>,
    pending: Option<NarrationJob>,
    _worker: JoinHandle<()>,
}

impl NarrationManager {
    /// Create a manager with a single narration worker thread.
    pub fn new() -> std::io::Result<Self> {
        let (job_tx, job_rx) = mpsc::channel::<NarrationJob>();
        let (event_tx, event_rx) = mpsc::channel::<UiEvent>();
        let worker = std::thread::Builder::new()
            .name("laudit-narration".to_owned())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                while let Ok(job) = job_rx.recv() {
                    run_job(&runtime, job, &event_tx);
                }
            })?;
        Ok(Self {
            job_tx,
            event_rx,
            active: None,
            pending: None,
            _worker: worker,
        })
    }

    /// Whether `stop_id` is currently being narrated.
    #[must_use]
    pub fn is_active(&self, stop_id: &str) -> bool {
        self.active.as_deref() == Some(stop_id)
    }

    /// Request narration for `job`, queueing it if another stop is active.
    pub fn request(&mut self, job: NarrationJob) {
        if self.active.is_some() {
            self.pending = Some(job);
        } else {
            self.start(job);
        }
    }

    /// Drop any queued narration request.
    pub fn cancel_pending(&mut self) {
        self.pending = None;
    }

    /// Drain events from the worker, starting a queued job when one finishes.
    pub fn poll(&mut self) -> Vec<UiEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.event_rx.try_recv() {
            let terminal = matches!(
                event,
                UiEvent::StopNarrated { .. } | UiEvent::NarrationFailed { .. }
            );
            events.push(event);
            if terminal {
                self.active = None;
                if let Some(job) = self.pending.take() {
                    self.start(job);
                }
            }
        }
        events
    }

    fn start(&mut self, job: NarrationJob) {
        self.active = Some(job.stop.id.clone());
        let _ = self.job_tx.send(job);
    }
}

fn run_job(runtime: &tokio::runtime::Runtime, job: NarrationJob, event_tx: &Sender<UiEvent>) {
    let mut sink = ChannelSink {
        tx: event_tx.clone(),
    };
    let mut stop = job.stop.clone();
    let result = runtime.block_on(narration::narrate_stop(
        &job.repo_root,
        &job.config,
        NarrationContext {
            skeleton: &job.skeleton,
            model: &job.model,
        },
        &job.chapter,
        &mut stop,
        &job.client,
        &mut sink,
    ));

    match result {
        Ok(report) => {
            let _ = event_tx.send(UiEvent::StopNarrated {
                stop: report.stop,
                warnings: report.warnings,
            });
        }
        Err(error) => {
            let _ = event_tx.send(UiEvent::NarrationFailed {
                stop,
                message: error.to_string(),
            });
        }
    }
}

struct ChannelSink {
    tx: Sender<UiEvent>,
}

impl NarrationSink for ChannelSink {
    fn on_chunk(&mut self, stop_id: &str, delta: &str) {
        let _ = self.tx.send(UiEvent::Chunk {
            stop_id: stop_id.to_owned(),
            delta: delta.to_owned(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_sink_forwards_chunks() {
        let (tx, rx) = mpsc::channel();
        let mut sink = ChannelSink { tx };

        sink.on_chunk("stop-1", "hello");

        match rx.try_recv().expect("event") {
            UiEvent::Chunk { stop_id, delta } => {
                assert_eq!(stop_id, "stop-1");
                assert_eq!(delta, "hello");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
