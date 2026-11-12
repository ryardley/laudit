use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::{mpsc, watch, Mutex};

use super::protocol::{error_response, success_response, ErrorCode, RpcError, JSONRPC_VERSION};
use crate::build;
use crate::comprehension;
use crate::config::{Config, ConfigError};
use crate::llm::{LlmClient, LlmError};
use crate::model::{CodebaseModel, Skeleton, Stop, Tour};
use crate::narration::{self, NarrationError, NarrationSink};
use crate::plan::{self, PlanError, PlanOptions};
use crate::status as tour_status;

pub type HandlerFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, RpcError>> + Send + 'a>>;
pub type Handler = for<'a> fn(&'a Value, &'a ServerContext) -> HandlerFuture<'a>;

#[derive(Clone)]
pub struct Client {
    tx: mpsc::UnboundedSender<Value>,
}

impl Client {
    pub fn notify(&self, method: &str, params: Value) -> Result<(), RpcError> {
        let message = json!({
            "jsonrpc": JSONRPC_VERSION,
            "method": method,
            "params": params,
        });
        self.tx
            .send(message)
            .map_err(|_| RpcError::internal("connection closed"))
    }
}

#[derive(Clone)]
struct Cancellation {
    tx: Arc<watch::Sender<bool>>,
}

impl Cancellation {
    fn new() -> (Self, watch::Receiver<bool>) {
        let (tx, rx) = watch::channel(false);
        (Self { tx: Arc::new(tx) }, rx)
    }

    fn cancel(&self) {
        let _ = self.tx.send(true);
    }
}

async fn wait_cancelled(mut rx: watch::Receiver<bool>) {
    if *rx.borrow() {
        return;
    }
    let _ = rx.changed().await;
}

struct ActiveTour {
    tour_id: String,
    repo: PathBuf,
    cancel: Cancellation,
    stops: HashMap<String, Cancellation>,
}

impl ActiveTour {
    fn cancel_all(&self) {
        self.cancel.cancel();
        for token in self.stops.values() {
            token.cancel();
        }
    }
}

struct ServerState {
    active: Mutex<Option<ActiveTour>>,
    next_id: AtomicU64,
}

impl ServerState {
    fn new() -> Self {
        Self {
            active: Mutex::new(None),
            next_id: AtomicU64::new(0),
        }
    }

    fn next_tour_id(&self, repo: &Path) -> String {
        let counter = self.next_id.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let key = format!("{}|{nanos}|{counter}", repo.display());
        let hash = crate::recon::content_hash(key.as_bytes());
        let hex = hash.strip_prefix("fnv1a128:").unwrap_or(&hash);
        let end = 16.min(hex.len());
        format!("tour-{}", &hex[..end])
    }
}

#[derive(Clone)]
pub struct ServerContext {
    version: &'static str,
    client: Client,
    state: Arc<ServerState>,
}

impl ServerContext {
    #[must_use]
    pub fn version(&self) -> &'static str {
        self.version
    }

    #[must_use]
    pub fn client(&self) -> &Client {
        &self.client
    }

    fn state(&self) -> &ServerState {
        &self.state
    }
}

struct MethodRegistry {
    handlers: HashMap<&'static str, Handler>,
}

impl MethodRegistry {
    fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    fn insert(&mut self, method: &'static str, handler: Handler) {
        self.handlers.insert(method, handler);
    }

    fn get(&self, method: &str) -> Option<Handler> {
        self.handlers.get(method).copied()
    }

    fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.insert("ping", ping);
        registry.insert("status", status);
        registry.insert("build_tour", build_tour);
        registry.insert("explain_stop", explain_stop);
        registry.insert("cancel", cancel);
        registry.insert("get_outline", get_outline);
        registry
    }
}

pub struct Server {
    registry: MethodRegistry,
    state: Arc<ServerState>,
}

impl Server {
    #[must_use]
    pub fn new() -> Self {
        Self {
            registry: MethodRegistry::with_defaults(),
            state: Arc::new(ServerState::new()),
        }
    }

    pub fn register(&mut self, method: &'static str, handler: Handler) -> &mut Self {
        self.registry.insert(method, handler);
        self
    }

    pub async fn serve(&self) -> io::Result<()> {
        let (tx, rx) = mpsc::unbounded_channel();
        let writer = tokio::spawn(write_loop(rx));
        let context = ServerContext {
            version: crate::VERSION,
            client: Client { tx: tx.clone() },
            state: Arc::clone(&self.state),
        };
        let stdin = tokio::io::stdin();
        let mut lines = BufReader::new(stdin).lines();

        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let Some(response) = handle_line(line, &self.registry, &context).await else {
                        continue;
                    };
                    if tx.send(response).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    {
                        let active = self.state.active.lock().await;
                        if let Some(tour) = active.as_ref() {
                            tour.cancel_all();
                        }
                    }
                    drop(context);
                    drop(tx);
                    let _ = writer.await;
                    return Err(error);
                }
            }
        }

        {
            let active = self.state.active.lock().await;
            if let Some(tour) = active.as_ref() {
                tour.cancel_all();
            }
        }
        drop(context);
        drop(tx);
        match writer.await {
            Ok(result) => result,
            Err(error) => Err(io::Error::other(error)),
        }
    }

    pub fn run(&self) -> io::Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(self.serve())
    }
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

pub async fn serve() -> io::Result<()> {
    Server::new().serve().await
}

pub fn run() -> io::Result<()> {
    Server::new().run()
}

async fn write_loop(mut rx: mpsc::UnboundedReceiver<Value>) -> io::Result<()> {
    let stdout = tokio::io::stdout();
    let mut writer = BufWriter::new(stdout);
    while let Some(message) = rx.recv().await {
        let line = serde_json::to_string(&message).map_err(io::Error::other)?;
        writer.write_all(line.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }
    writer.flush().await?;
    Ok(())
}

async fn handle_line(
    line: &str,
    registry: &MethodRegistry,
    context: &ServerContext,
) -> Option<Value> {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return Some(error_response(
                Value::Null,
                &RpcError::parse_error(error.to_string()),
            ));
        }
    };

    match value {
        Value::Array(items) => handle_batch(items, registry, context).await,
        Value::Object(_) => handle_single(value, registry, context).await,
        _ => Some(error_response(
            Value::Null,
            &RpcError::invalid_request("request must be a JSON object"),
        )),
    }
}

async fn handle_batch(
    items: Vec<Value>,
    registry: &MethodRegistry,
    context: &ServerContext,
) -> Option<Value> {
    if items.is_empty() {
        return Some(error_response(
            Value::Null,
            &RpcError::invalid_request("batch must not be empty"),
        ));
    }

    let mut responses = Vec::new();
    for item in items {
        if let Some(response) = handle_single(item, registry, context).await {
            responses.push(response);
        }
    }

    if responses.is_empty() {
        None
    } else {
        Some(Value::Array(responses))
    }
}

async fn handle_single(
    value: Value,
    registry: &MethodRegistry,
    context: &ServerContext,
) -> Option<Value> {
    let Value::Object(mut object) = value else {
        return Some(error_response(
            Value::Null,
            &RpcError::invalid_request("request must be a JSON object"),
        ));
    };

    let id = object.remove("id");
    if let Some(id_value) = &id {
        match id_value {
            Value::String(_) | Value::Number(_) | Value::Null => {}
            _ => {
                return Some(error_response(
                    Value::Null,
                    &RpcError::invalid_request("id must be a string, number, or null"),
                ));
            }
        }
    }
    let is_notification = id.is_none();
    let response_id = id.unwrap_or(Value::Null);

    match object.remove("jsonrpc") {
        Some(Value::String(version)) if version == JSONRPC_VERSION => {}
        _ => {
            return Some(error_response(
                response_id,
                &RpcError::invalid_request("jsonrpc must be \"2.0\""),
            ));
        }
    }

    let method = match object.remove("method") {
        Some(Value::String(method)) => method,
        _ => {
            return Some(error_response(
                response_id,
                &RpcError::invalid_request("method must be a string"),
            ));
        }
    };

    let params = match object.remove("params") {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(params @ (Value::Object(_) | Value::Array(_))) => params,
        Some(_) => {
            if is_notification {
                return None;
            }
            return Some(error_response(
                response_id,
                &RpcError::invalid_params("params must be an object or array"),
            ));
        }
    };

    let Some(handler) = registry.get(&method) else {
        if is_notification {
            return None;
        }
        return Some(error_response(
            response_id,
            &RpcError::method_not_found(&method),
        ));
    };

    match handler(&params, context).await {
        Ok(result) => {
            if is_notification {
                None
            } else {
                Some(success_response(response_id, result))
            }
        }
        Err(error) => {
            if is_notification {
                None
            } else {
                Some(error_response(response_id, &error))
            }
        }
    }
}

fn ping<'a>(_params: &'a Value, context: &'a ServerContext) -> HandlerFuture<'a> {
    Box::pin(async move {
        Ok(json!({
            "pong": true,
            "version": context.version(),
        }))
    })
}

fn status<'a>(params: &'a Value, context: &'a ServerContext) -> HandlerFuture<'a> {
    Box::pin(async move {
        let tour_id = optional_string(params, "tour_id")?;
        let mut result = json!({ "engine": "ok" });
        let active = context.state().active.lock().await;
        if let Some(tour) = active.as_ref() {
            if tour_id.as_deref().is_none_or(|id| id == tour.tour_id) {
                if let Ok(report) = tour_status::status(&tour.repo, Some(&tour.tour_id)) {
                    if let Ok(value) = serde_json::to_value(&report) {
                        result["tour"] = value;
                    }
                }
            }
        }
        Ok(result)
    })
}

fn build_tour<'a>(params: &'a Value, context: &'a ServerContext) -> HandlerFuture<'a> {
    Box::pin(async move {
        let repo = required_string(params, "repo")?;
        let offline = optional_bool(params, "offline")?.unwrap_or(false);
        if let Some(source) = params.get("source") {
            if !source.is_null() && !source.is_object() {
                return Err(RpcError::invalid_params("source must be an object"));
            }
        }

        let repo = PathBuf::from(repo);
        if !repo.is_dir() {
            return Err(RpcError::invalid_params(format!(
                "repo is not a directory: {}",
                repo.display()
            )));
        }

        let config = if offline {
            Config::load_offline(&repo).map_err(map_config_error)?
        } else {
            Config::load(&repo).map_err(map_config_error)?
        };
        let client = if offline {
            None
        } else {
            Some(LlmClient::from_config(&config).map_err(map_llm_config_error)?)
        };

        let tour_id = context.state().next_tour_id(&repo);
        let (cancel, cancel_rx) = Cancellation::new();
        {
            let mut active = context.state().active.lock().await;
            if let Some(previous) = active.as_ref() {
                previous.cancel_all();
            }
            *active = Some(ActiveTour {
                tour_id: tour_id.clone(),
                repo: repo.clone(),
                cancel,
                stops: HashMap::new(),
            });
        }

        let context = context.clone();
        let task_tour_id = tour_id.clone();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            let result = tokio::select! {
                _ = wait_cancelled(cancel_rx) => {
                    Err(RpcError::new(ErrorCode::Cancelled, "cancelled"))
                }
                result = run_build_tour(
                    &context,
                    &repo,
                    &config,
                    client.as_ref(),
                    &task_tour_id,
                    offline,
                ) => result,
            };
            if let Err(error) = result {
                let _ = context.client().notify(
                    "error",
                    json!({
                        "tour_id": task_tour_id,
                        "code": error.code.code(),
                        "message": error.message,
                    }),
                );
            }
        });

        Ok(json!({ "tour_id": tour_id }))
    })
}

fn explain_stop<'a>(params: &'a Value, context: &'a ServerContext) -> HandlerFuture<'a> {
    Box::pin(async move {
        let tour_id = required_string(params, "tour_id")?;
        let stop_id = required_string(params, "stop_id")?;

        let repo = {
            let mut active = context.state().active.lock().await;
            let Some(tour) = active.as_mut() else {
                return Err(tour_not_found(&tour_id));
            };
            if tour.tour_id != tour_id {
                return Err(tour_not_found(&tour_id));
            }
            let (cancel, cancel_rx) = Cancellation::new();
            if let Some(previous) = tour.stops.insert(stop_id.clone(), cancel) {
                previous.cancel();
            }
            (tour.repo.clone(), cancel_rx)
        };
        let (repo, cancel_rx) = repo;

        let config = Config::load(&repo).map_err(map_config_error)?;
        let client = LlmClient::from_config(&config).map_err(map_llm_config_error)?;
        let skeleton = plan::load_skeleton(&repo).map_err(map_plan_error)?;
        let model = plan::load_model(&repo).map_err(map_plan_error)?;
        let mut tour = plan::load_tour(&repo, &tour_id).map_err(map_plan_error)?;
        let (chapter_index, stop_index) =
            find_stop(&tour, &stop_id).ok_or_else(|| stop_not_found(&stop_id))?;

        let context = context.clone();
        let task_tour_id = tour_id.clone();
        let task_stop_id = stop_id.clone();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            let result = tokio::select! {
                _ = wait_cancelled(cancel_rx) => {
                    Err(RpcError::new(ErrorCode::Cancelled, "cancelled"))
                }
                result = run_explain_stop(ExplainJob {
                    context: &context,
                    repo: &repo,
                    config: &config,
                    skeleton: &skeleton,
                    model: &model,
                    tour: &mut tour,
                    chapter_index,
                    stop_index,
                    client: &client,
                    tour_id: &task_tour_id,
                }) => result,
            };
            if let Err(error) = result {
                let _ = context.client().notify(
                    "error",
                    json!({
                        "tour_id": task_tour_id,
                        "stop_id": task_stop_id,
                        "code": error.code.code(),
                        "message": error.message,
                    }),
                );
            }
            let mut active = context.state().active.lock().await;
            if let Some(tour) = active.as_mut() {
                if tour.tour_id == task_tour_id {
                    tour.stops.remove(&task_stop_id);
                }
            }
        });

        Ok(json!({ "stop_id": stop_id }))
    })
}

fn cancel<'a>(params: &'a Value, context: &'a ServerContext) -> HandlerFuture<'a> {
    Box::pin(async move {
        let tour_id = required_string(params, "tour_id")?;
        let stop_id = optional_string(params, "stop_id")?;
        let active = context.state().active.lock().await;
        let Some(tour) = active.as_ref() else {
            return Err(tour_not_found(&tour_id));
        };
        if tour.tour_id != tour_id {
            return Err(tour_not_found(&tour_id));
        }
        if let Some(stop_id) = stop_id {
            if let Some(token) = tour.stops.get(&stop_id) {
                token.cancel();
            } else {
                let stop_exists = plan::load_tour(&tour.repo, &tour.tour_id)
                    .ok()
                    .is_none_or(|planned| find_stop(&planned, &stop_id).is_some());
                if !stop_exists {
                    return Err(stop_not_found(&stop_id));
                }
                tour.cancel.cancel();
            }
        } else {
            tour.cancel_all();
        }
        Ok(json!({ "cancelled": true }))
    })
}

fn get_outline<'a>(params: &'a Value, context: &'a ServerContext) -> HandlerFuture<'a> {
    Box::pin(async move {
        let tour_id = required_string(params, "tour_id")?;
        let repo = {
            let active = context.state().active.lock().await;
            let Some(tour) = active.as_ref() else {
                return Err(tour_not_found(&tour_id));
            };
            if tour.tour_id != tour_id {
                return Err(tour_not_found(&tour_id));
            }
            tour.repo.clone()
        };
        if !plan::tour_path(&repo, &tour_id).is_file() {
            return Err(tour_not_found(&tour_id));
        }
        let tour = plan::load_tour(&repo, &tour_id).map_err(map_plan_error)?;
        Ok(outline_value(&tour))
    })
}

async fn run_build_tour(
    context: &ServerContext,
    repo: &Path,
    config: &Config,
    client: Option<&LlmClient>,
    tour_id: &str,
    offline: bool,
) -> Result<(), RpcError> {
    notify_progress(context, tour_id, "recon", "Scanning repository", Some(0));
    let recon = build::recon(repo, config).map_err(map_build_error)?;
    build::write_skeleton(repo, &recon.skeleton).map_err(map_build_error)?;
    notify_progress(context, tour_id, "recon", "Recon complete", Some(100));

    let model = if offline {
        let model = build::offline_model(&recon.skeleton);
        build::write_model(repo, &model).map_err(map_build_error)?;
        model
    } else {
        notify_progress(
            context,
            tour_id,
            "comprehension",
            "Building codebase model",
            Some(0),
        );
        let client = client.ok_or_else(no_model_configured)?;
        let report = comprehension::comprehend(repo, config, &recon.skeleton, client)
            .await
            .map_err(map_comprehension_error)?;
        let _ = context.client().notify(
            "comprehension_done",
            json!({
                "tour_id": tour_id,
                "model_path": report.model_path.to_string_lossy(),
            }),
        );
        notify_progress(
            context,
            tour_id,
            "comprehension",
            "Codebase model ready",
            Some(100),
        );
        report.model
    };

    notify_progress(context, tour_id, "plan", "Planning tour", Some(0));
    let plan_report = plan::plan_with_tour_id(
        repo,
        config,
        &recon.skeleton,
        &model,
        &PlanOptions::default(),
        tour_id,
    )
    .map_err(map_plan_error)?;
    notify_progress(context, tour_id, "plan", "Tour planned", Some(100));

    if offline {
        for chapter in &plan_report.tour.chapters {
            for stop in &chapter.stops {
                notify_stop_ready(context.client(), tour_id, stop);
            }
        }
        return Ok(());
    }

    let client = client.ok_or_else(no_model_configured)?;
    notify_progress(context, tour_id, "narration", "Narrating stops", Some(0));
    let mut tour = plan_report.tour;
    let mut sink = RpcNarrationSink {
        client: context.client().clone(),
        tour_id: tour_id.to_owned(),
    };
    narration::narrate(
        repo,
        config,
        &recon.skeleton,
        &model,
        &mut tour,
        client,
        &mut sink,
    )
    .await
    .map_err(map_narration_error)?;
    notify_progress(context, tour_id, "narration", "Tour ready", Some(100));
    Ok(())
}

struct ExplainJob<'a> {
    context: &'a ServerContext,
    repo: &'a Path,
    config: &'a Config,
    skeleton: &'a Skeleton,
    model: &'a CodebaseModel,
    tour: &'a mut Tour,
    chapter_index: usize,
    stop_index: usize,
    client: &'a LlmClient,
    tour_id: &'a str,
}

async fn run_explain_stop(job: ExplainJob<'_>) -> Result<(), RpcError> {
    let chapter = job.tour.chapters[job.chapter_index].clone();
    let mut stop = chapter.stops[job.stop_index].clone();
    let mut sink = RpcNarrationSink {
        client: job.context.client().clone(),
        tour_id: job.tour_id.to_owned(),
    };
    let report = narration::narrate_stop(
        job.repo,
        job.config,
        narration::NarrationContext {
            skeleton: job.skeleton,
            model: job.model,
        },
        &chapter,
        &mut stop,
        job.client,
        &mut sink,
    )
    .await
    .map_err(map_narration_error)?;
    job.tour.chapters[job.chapter_index].stops[job.stop_index] = stop.clone();
    job.tour.model = job.client.model().to_owned();
    job.tour.prompt_version = report.prompt_version;
    plan::write_tour(job.repo, job.tour).map_err(map_plan_error)?;
    notify_stop_ready(job.context.client(), job.tour_id, &stop);
    Ok(())
}

struct RpcNarrationSink {
    client: Client,
    tour_id: String,
}

impl NarrationSink for RpcNarrationSink {
    fn on_chunk(&mut self, stop_id: &str, delta: &str) {
        let _ = self.client.notify(
            "explanation_chunk",
            json!({
                "tour_id": self.tour_id,
                "stop_id": stop_id,
                "delta": delta,
            }),
        );
    }

    fn on_stop(&mut self, stop: &Stop) {
        notify_stop_ready(&self.client, &self.tour_id, stop);
    }
}

fn notify_progress(
    context: &ServerContext,
    tour_id: &str,
    stage: &str,
    message: &str,
    pct: Option<u32>,
) {
    let mut params = json!({
        "tour_id": tour_id,
        "stage": stage,
        "message": message,
    });
    if let Some(pct) = pct {
        params["pct"] = json!(pct);
    }
    let _ = context.client().notify("progress", params);
}

fn notify_stop_ready(client: &Client, tour_id: &str, stop: &Stop) {
    let _ = client.notify(
        "stop_ready",
        json!({
            "tour_id": tour_id,
            "stop_id": stop.id,
            "file": stop.file,
            "start_line": stop.start_line,
            "end_line": stop.end_line,
            "title": stop.title,
            "why_here": stop.why_here,
        }),
    );
}

fn outline_value(tour: &Tour) -> Value {
    json!({
        "chapters": tour.chapters.iter().map(|chapter| json!({
            "id": chapter.id,
            "title": chapter.title,
            "stops": chapter.stops.iter().map(|stop| json!({
                "id": stop.id,
                "title": stop.title,
                "file": stop.file,
                "start_line": stop.start_line,
                "end_line": stop.end_line,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn find_stop(tour: &Tour, stop_id: &str) -> Option<(usize, usize)> {
    for (chapter_index, chapter) in tour.chapters.iter().enumerate() {
        for (stop_index, stop) in chapter.stops.iter().enumerate() {
            if stop.id == stop_id {
                return Some((chapter_index, stop_index));
            }
        }
    }
    None
}

fn required_string(params: &Value, name: &str) -> Result<String, RpcError> {
    match params.get(name) {
        Some(Value::String(value)) if !value.is_empty() => Ok(value.clone()),
        Some(Value::String(_)) => Err(RpcError::invalid_params(format!(
            "{name} must not be empty"
        ))),
        Some(_) => Err(RpcError::invalid_params(format!("{name} must be a string"))),
        None => Err(RpcError::invalid_params(format!("{name} is required"))),
    }
}

fn optional_string(params: &Value, name: &str) -> Result<Option<String>, RpcError> {
    match params.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.is_empty() => Ok(Some(value.clone())),
        Some(Value::String(_)) => Err(RpcError::invalid_params(format!(
            "{name} must not be empty"
        ))),
        Some(_) => Err(RpcError::invalid_params(format!("{name} must be a string"))),
    }
}

fn optional_bool(params: &Value, name: &str) -> Result<Option<bool>, RpcError> {
    match params.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(RpcError::invalid_params(format!(
            "{name} must be a boolean"
        ))),
    }
}

fn tour_not_found(tour_id: &str) -> RpcError {
    RpcError::new(
        ErrorCode::TourNotFound,
        format!("tour not found: {tour_id}"),
    )
}

fn stop_not_found(stop_id: &str) -> RpcError {
    RpcError::new(
        ErrorCode::StopNotFound,
        format!("stop not found: {stop_id}"),
    )
}

fn no_model_configured() -> RpcError {
    RpcError::new(ErrorCode::NoModelConfigured, "no model configured")
}

fn map_config_error(error: ConfigError) -> RpcError {
    match error {
        ConfigError::MissingModel => RpcError::new(ErrorCode::NoModelConfigured, error.to_string()),
        other => RpcError::internal(other.to_string()),
    }
}

fn map_llm_config_error(error: LlmError) -> RpcError {
    match error {
        LlmError::MissingModel | LlmError::MissingApiKey { .. } => {
            RpcError::new(ErrorCode::NoModelConfigured, error.to_string())
        }
        other => RpcError::internal(other.to_string()),
    }
}

fn map_build_error(error: build::BuildError) -> RpcError {
    RpcError::internal(error.to_string())
}

fn map_comprehension_error(error: comprehension::ComprehensionError) -> RpcError {
    match error {
        comprehension::ComprehensionError::Llm(_) => {
            RpcError::new(ErrorCode::LlmFailure, error.to_string())
        }
        other => RpcError::internal(other.to_string()),
    }
}

fn map_plan_error(error: PlanError) -> RpcError {
    RpcError::internal(error.to_string())
}

fn map_narration_error(error: NarrationError) -> RpcError {
    match error {
        NarrationError::StopFailed(_) => RpcError::new(ErrorCode::LlmFailure, error.to_string()),
        other => RpcError::internal(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> ServerContext {
        let (tx, _rx) = mpsc::unbounded_channel();
        ServerContext {
            version: crate::VERSION,
            client: Client { tx },
            state: Arc::new(ServerState::new()),
        }
    }

    #[tokio::test]
    async fn ping_returns_pong_and_version() {
        let response = handle_line(
            r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#,
            &MethodRegistry::with_defaults(),
            &context(),
        )
        .await
        .expect("response");

        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 1);
        assert_eq!(response["result"]["pong"], true);
        assert_eq!(response["result"]["version"], crate::VERSION);
    }

    #[tokio::test]
    async fn status_reports_engine_ok() {
        let response = handle_line(
            r#"{"jsonrpc":"2.0","id":"s1","method":"status","params":{}}"#,
            &MethodRegistry::with_defaults(),
            &context(),
        )
        .await
        .expect("response");

        assert_eq!(response["id"], "s1");
        assert_eq!(response["result"]["engine"], "ok");
    }

    #[tokio::test]
    async fn malformed_json_returns_parse_error() {
        let response = handle_line("not json", &MethodRegistry::with_defaults(), &context())
            .await
            .expect("response");

        assert!(response["id"].is_null());
        assert_eq!(response["error"]["code"], -32700);
    }

    #[tokio::test]
    async fn unknown_method_returns_method_not_found() {
        let response = handle_line(
            r#"{"jsonrpc":"2.0","id":2,"method":"nope","params":{}}"#,
            &MethodRegistry::with_defaults(),
            &context(),
        )
        .await
        .expect("response");

        assert_eq!(response["id"], 2);
        assert_eq!(response["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn notifications_do_not_produce_responses() {
        let response = handle_line(
            r#"{"jsonrpc":"2.0","method":"ping","params":{}}"#,
            &MethodRegistry::with_defaults(),
            &context(),
        )
        .await;

        assert!(response.is_none());
    }

    #[tokio::test]
    async fn client_notify_emits_notification() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let client = Client { tx };

        client
            .notify("progress", json!({ "pct": 50 }))
            .expect("notify");

        let message = rx.recv().await.expect("notification");
        assert_eq!(message["jsonrpc"], "2.0");
        assert_eq!(message["method"], "progress");
        assert_eq!(message["params"]["pct"], 50);
        assert!(message.get("id").is_none());
    }
}
