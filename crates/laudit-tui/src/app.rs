//! Tour state and key handling for the terminal frontend.
//!
//! [`App`] owns the loaded tour and the current position. It is deliberately
//! independent of the terminal so navigation, outline selection, and code
//! loading can be unit tested without a TTY.

use std::fs;
use std::io;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use laudit_core::build;
use laudit_core::cache;
use laudit_core::config::Config;
use laudit_core::explain;
use laudit_core::llm::LlmClient;
use laudit_core::model::{CodebaseModel, Position, Skeleton, Stop, StopStatus, Tour};
use laudit_core::plan::{self, PlanOptions};
use laudit_core::status;

use crate::narration::{NarrationJob, NarrationManager, UiEvent};
use crate::TuiError;

/// Everything the TUI needs to render and narrate a tour.
#[derive(Debug, Clone)]
pub struct Session {
    /// Repository root.
    pub repo_root: PathBuf,
    /// Resolved engine configuration.
    pub config: Config,
    /// Recon skeleton.
    pub skeleton: Skeleton,
    /// Codebase model.
    pub model: CodebaseModel,
    /// The tour being rendered.
    pub tour: Tour,
    /// LLM client, absent in offline mode.
    pub client: Option<LlmClient>,
    /// Whether narration is disabled.
    pub offline: bool,
}

/// Load the tour artifact, planning one when none exists.
///
/// This mirrors the `laudit tour --print` path: the skeleton is loaded (running
/// recon if needed), the model is loaded (falling back to the offline
/// placeholder in offline mode), and stale stops are invalidated before the
/// tour is shown.
pub fn load_session(repo_root: PathBuf, offline: bool) -> Result<Session, TuiError> {
    let config = if offline {
        Config::load_offline(&repo_root).map_err(TuiError::msg)?
    } else {
        Config::load(&repo_root).map_err(TuiError::msg)?
    };
    let client = if offline {
        None
    } else {
        Some(LlmClient::from_config(&config).map_err(TuiError::msg)?)
    };

    let skeleton = explain::load_skeleton(&repo_root, &config).map_err(TuiError::msg)?;
    let model = match plan::load_model(&repo_root) {
        Ok(model) => model,
        Err(plan::PlanError::Io { source, .. })
            if offline && source.kind() == io::ErrorKind::NotFound =>
        {
            build::offline_model(&skeleton)
        }
        Err(error) => return Err(TuiError::msg(error)),
    };

    let mut tour = match status::latest_tour_id(&repo_root).map_err(TuiError::msg)? {
        Some(tour_id) => plan::load_tour(&repo_root, &tour_id).map_err(TuiError::msg)?,
        None => {
            plan::plan(
                &repo_root,
                &config,
                &skeleton,
                &model,
                &PlanOptions::default(),
            )
            .map_err(TuiError::msg)?
            .tour
        }
    };

    let invalidation = cache::invalidate_stale_stops(&repo_root, &mut tour);
    if !invalidation.is_empty() {
        plan::write_tour(&repo_root, &tour).map_err(TuiError::msg)?;
    }

    Ok(Session {
        repo_root,
        config,
        skeleton,
        model,
        tour,
        client,
        offline,
    })
}

/// A row in the outline overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineRow {
    /// Chapter title.
    pub chapter_title: String,
    /// Stop title.
    pub stop_title: String,
    /// File containing the stop.
    pub file: String,
    /// First line of the stop.
    pub line: u32,
    /// Flat stop index.
    pub flat_index: usize,
}

/// Mutable tour state for the terminal frontend.
#[derive(Debug)]
pub struct App {
    pub(crate) repo_root: PathBuf,
    pub(crate) config: Config,
    pub(crate) skeleton: Skeleton,
    pub(crate) model: CodebaseModel,
    pub(crate) tour: Tour,
    pub(crate) client: Option<LlmClient>,
    pub(crate) offline: bool,
    pub(crate) current: usize,
    pub(crate) outline_open: bool,
    pub(crate) outline_selected: usize,
    pub(crate) code_lines: Vec<String>,
    pub(crate) code_scroll: u16,
    pub(crate) explanation: String,
    pub(crate) explanation_scroll: u16,
    pub(crate) follow_explanation: bool,
    pub(crate) status: String,
    pub(crate) warnings: Vec<String>,
    pub(crate) quit: bool,
    pub(crate) dirty: bool,
}

impl App {
    /// Build an app from a loaded session, restoring the saved position.
    #[must_use]
    pub fn new(session: Session) -> Self {
        let mut app = Self {
            repo_root: session.repo_root,
            config: session.config,
            skeleton: session.skeleton,
            model: session.model,
            tour: session.tour,
            client: session.client,
            offline: session.offline,
            current: 0,
            outline_open: false,
            outline_selected: 0,
            code_lines: Vec::new(),
            code_scroll: 0,
            explanation: String::new(),
            explanation_scroll: 0,
            follow_explanation: true,
            status: String::new(),
            warnings: Vec::new(),
            quit: false,
            dirty: true,
        };
        let start = app.saved_position_index();
        app.set_current(start);
        app
    }

    /// The current stop, if the tour has any stops.
    #[must_use]
    pub fn current_stop(&self) -> Option<&Stop> {
        let (chapter, stop) = self.current_location()?;
        self.tour.chapters.get(chapter)?.stops.get(stop)
    }

    /// The current chapter and stop indices.
    #[must_use]
    pub fn current_location(&self) -> Option<(usize, usize)> {
        let mut flat = 0;
        for (chapter_index, chapter) in self.tour.chapters.iter().enumerate() {
            for (stop_index, _) in chapter.stops.iter().enumerate() {
                if flat == self.current {
                    return Some((chapter_index, stop_index));
                }
                flat += 1;
            }
        }
        None
    }

    /// Total number of stops in the tour.
    #[must_use]
    pub fn total_stops(&self) -> usize {
        self.tour
            .chapters
            .iter()
            .map(|chapter| chapter.stops.len())
            .sum()
    }

    /// Header progress as `(chapter, chapter_count, stop, stop_count)`.
    #[must_use]
    pub fn progress(&self) -> (usize, usize, usize, usize) {
        let chapter_count = self.tour.chapters.len();
        let Some((chapter, stop)) = self.current_location() else {
            return (0, chapter_count, 0, 0);
        };
        let stop_count = self.tour.chapters[chapter].stops.len();
        (chapter + 1, chapter_count, stop + 1, stop_count)
    }

    /// Rows for the outline overlay.
    #[must_use]
    pub fn outline_rows(&self) -> Vec<OutlineRow> {
        let mut rows = Vec::new();
        let mut flat = 0;
        for chapter in &self.tour.chapters {
            for stop in &chapter.stops {
                rows.push(OutlineRow {
                    chapter_title: chapter.title.clone(),
                    stop_title: stop_title(stop),
                    file: stop.file.clone(),
                    line: stop.start_line,
                    flat_index: flat,
                });
                flat += 1;
            }
        }
        rows
    }

    /// Move to the next stop, clamped at the end.
    pub fn next(&mut self) {
        let total = self.total_stops();
        if total > 0 && self.current + 1 < total {
            self.set_current(self.current + 1);
        }
    }

    /// Move to the previous stop, clamped at the start.
    pub fn prev(&mut self) {
        if self.current > 0 {
            self.set_current(self.current - 1);
        }
    }

    /// Snap the code pane back to the current stop and close the outline.
    pub fn return_to_current(&mut self) {
        self.outline_open = false;
        self.load_code();
        self.follow_explanation = true;
        self.dirty = true;
    }

    /// Toggle the outline overlay.
    pub fn toggle_outline(&mut self) {
        self.outline_open = !self.outline_open;
        if self.outline_open {
            self.outline_selected = self.current;
        }
        self.dirty = true;
    }

    /// Move the outline selection down.
    pub fn outline_next(&mut self) {
        let total = self.total_stops();
        if total > 0 {
            self.outline_selected = (self.outline_selected + 1).min(total - 1);
        }
        self.dirty = true;
    }

    /// Move the outline selection up.
    pub fn outline_prev(&mut self) {
        self.outline_selected = self.outline_selected.saturating_sub(1);
        self.dirty = true;
    }

    /// Jump to the selected outline row.
    pub fn outline_select(&mut self) {
        let index = self.outline_selected;
        self.outline_open = false;
        self.set_current(index);
    }

    /// Handle a key press.
    pub fn handle_key(&mut self, key: KeyEvent, narration: &mut NarrationManager) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }

        if self.outline_open {
            match key.code {
                KeyCode::Char('q') => self.quit = true,
                KeyCode::Esc | KeyCode::Char('o') => {
                    self.outline_open = false;
                    self.dirty = true;
                }
                KeyCode::Char('j') | KeyCode::Down => self.outline_next(),
                KeyCode::Char('k') | KeyCode::Up => self.outline_prev(),
                KeyCode::Enter => self.outline_select(),
                KeyCode::Char('g') => {
                    self.outline_open = false;
                    self.return_to_current();
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('n') => {
                self.next();
                self.request_current_narration(narration);
            }
            KeyCode::Char('p') => {
                self.prev();
                self.request_current_narration(narration);
            }
            KeyCode::Char('o') => self.toggle_outline(),
            KeyCode::Char('g') => self.return_to_current(),
            KeyCode::Char('r') => self.retry_current_narration(narration),
            KeyCode::Char('j') | KeyCode::Down => {
                self.code_scroll = self.code_scroll.saturating_add(1);
                self.dirty = true;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.code_scroll = self.code_scroll.saturating_sub(1);
                self.dirty = true;
            }
            KeyCode::PageDown => {
                self.code_scroll = self.code_scroll.saturating_add(10);
                self.dirty = true;
            }
            KeyCode::PageUp => {
                self.code_scroll = self.code_scroll.saturating_sub(10);
                self.dirty = true;
            }
            KeyCode::Home => {
                self.code_scroll = 0;
                self.dirty = true;
            }
            KeyCode::End => {
                self.code_scroll =
                    u16::try_from(self.code_lines.len().saturating_sub(1)).unwrap_or(u16::MAX);
                self.dirty = true;
            }
            _ => {}
        }
    }

    /// Start narrating the current stop if it is not already explained.
    pub fn request_current_narration(&mut self, narration: &mut NarrationManager) {
        if self.offline || self.client.is_none() {
            return;
        }
        let Some(stop) = self.current_stop() else {
            return;
        };
        if stop.status == StopStatus::Explained {
            narration.cancel_pending();
            return;
        }
        if narration.is_active(&stop.id) {
            return;
        }
        if let Some(job) = self.build_job() {
            narration.request(job);
            self.status = "Narrating...".to_owned();
            self.dirty = true;
        }
    }

    /// Apply a narration event from the background worker.
    pub fn handle_narration_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::Chunk { stop_id, delta } => {
                if self.current_stop().is_some_and(|stop| stop.id == stop_id) {
                    self.explanation.push_str(&delta);
                    self.follow_explanation = true;
                    self.dirty = true;
                }
            }
            UiEvent::StopNarrated { stop, warnings } => {
                let is_current = self
                    .current_stop()
                    .is_some_and(|current| current.id == stop.id);
                self.update_stop(stop.clone());
                self.warnings.extend(warnings);
                if is_current {
                    self.explanation = stop.explanation.clone();
                    self.status = "Explained".to_owned();
                    self.follow_explanation = true;
                }
                self.dirty = true;
            }
            UiEvent::NarrationFailed { stop, message } => {
                let is_current = self
                    .current_stop()
                    .is_some_and(|current| current.id == stop.id);
                self.update_stop(stop);
                if is_current {
                    self.status = format!("Narration failed: {message}");
                }
                self.dirty = true;
            }
        }
    }

    fn retry_current_narration(&mut self, narration: &mut NarrationManager) {
        if self.offline || self.client.is_none() {
            return;
        }
        let Some((chapter, stop)) = self.current_location() else {
            return;
        };
        {
            let stop = &mut self.tour.chapters[chapter].stops[stop];
            stop.status = StopStatus::Pending;
            stop.explanation.clear();
            stop.refs.clear();
        }
        self.explanation.clear();
        self.status = "Narrating...".to_owned();
        self.follow_explanation = true;
        self.write_tour();
        if let Some(job) = self.build_job() {
            narration.request(job);
        }
        self.dirty = true;
    }

    fn build_job(&self) -> Option<NarrationJob> {
        let (chapter_index, stop_index) = self.current_location()?;
        let chapter = self.tour.chapters.get(chapter_index)?.clone();
        let stop = chapter.stops.get(stop_index)?.clone();
        let client = self.client.clone()?;
        Some(NarrationJob {
            repo_root: self.repo_root.clone(),
            config: self.config.clone(),
            skeleton: self.skeleton.clone(),
            model: self.model.clone(),
            chapter,
            stop,
            client,
        })
    }

    fn saved_position_index(&self) -> usize {
        let mut flat = 0;
        for (chapter_index, chapter) in self.tour.chapters.iter().enumerate() {
            for (stop_index, _) in chapter.stops.iter().enumerate() {
                if chapter_index as u32 == self.tour.position.chapter
                    && stop_index as u32 == self.tour.position.stop
                {
                    return flat;
                }
                flat += 1;
            }
        }
        0
    }

    fn set_current(&mut self, index: usize) {
        let total = self.total_stops();
        if total == 0 {
            return;
        }
        self.current = index.min(total - 1);
        self.load_code();

        let Some((chapter_index, stop_index)) = self.current_location() else {
            return;
        };
        self.tour.position = Position {
            chapter: chapter_index as u32,
            stop: stop_index as u32,
        };
        let (explanation, status) = {
            let stop = &self.tour.chapters[chapter_index].stops[stop_index];
            (stop.explanation.clone(), status_text(stop.status))
        };
        self.explanation = explanation;
        self.status = status;
        self.follow_explanation = true;
        self.explanation_scroll = 0;
        self.write_tour();
        self.dirty = true;
    }

    fn load_code(&mut self) {
        self.code_lines.clear();
        let Some((chapter_index, stop_index)) = self.current_location() else {
            return;
        };
        let (file, start_line) = {
            let stop = &self.tour.chapters[chapter_index].stops[stop_index];
            (stop.file.clone(), stop.start_line)
        };
        let path = self.repo_root.join(&file);
        match fs::read_to_string(&path) {
            Ok(text) => {
                self.code_lines = text
                    .lines()
                    .map(|line| line.replace('\t', "    "))
                    .collect();
            }
            Err(error) => {
                self.code_lines = vec![format!("(failed to read {file}: {error})")];
            }
        }
        self.code_scroll =
            u16::try_from(start_line.saturating_sub(1).saturating_sub(2)).unwrap_or(u16::MAX);
    }

    fn update_stop(&mut self, stop: Stop) {
        let mut found = false;
        for chapter in &mut self.tour.chapters {
            for existing in &mut chapter.stops {
                if existing.id == stop.id {
                    *existing = stop.clone();
                    found = true;
                    break;
                }
            }
            if found {
                break;
            }
        }
        if found {
            self.write_tour();
        }
    }

    fn write_tour(&mut self) {
        if let Err(error) = plan::write_tour(&self.repo_root, &self.tour) {
            self.warnings.push(format!("failed to save tour: {error}"));
        }
    }
}

fn stop_title(stop: &Stop) -> String {
    if stop.title.is_empty() {
        stop.symbol_id.clone()
    } else {
        stop.title.clone()
    }
}

fn status_text(status: StopStatus) -> String {
    match status {
        StopStatus::Explained => "Explained".to_owned(),
        StopStatus::Skipped => "Skipped".to_owned(),
        StopStatus::Failed => "Failed".to_owned(),
        StopStatus::Streaming => "Narrating...".to_owned(),
        StopStatus::Pending => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use laudit_core::model::{TourChapter, TourSource, SCHEMA_VERSION};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("laudit-tui-test-{}-{id}", std::process::id()));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &PathBuf {
            &self.path
        }

        fn write(&self, name: &str, contents: &str) {
            let path = self.path.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(path, contents).expect("write file");
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn stop(id: &str, file: &str, start_line: u32, end_line: u32) -> Stop {
        Stop {
            id: id.to_owned(),
            file: file.to_owned(),
            start_line,
            end_line,
            symbol_id: format!("sym-{id}"),
            title: id.to_owned(),
            why_here: "why".to_owned(),
            explanation: String::new(),
            refs: Vec::new(),
            file_hash: String::new(),
            status: StopStatus::Pending,
        }
    }

    fn session(temp: &TempDir) -> Session {
        let skeleton = Skeleton {
            schema_version: SCHEMA_VERSION,
            tool_version: "0.1.0".to_owned(),
            repo_id: "test".to_owned(),
            head: None,
            dirty_hash: "test".to_owned(),
            files: Vec::new(),
            symbols: Vec::new(),
            communities: Vec::new(),
            entrypoints: Vec::new(),
            ranked_files: Vec::new(),
            ranked_symbols: Vec::new(),
            provenance: BTreeMap::new(),
        };
        let model = build::offline_model(&skeleton);
        let tour = Tour {
            schema_version: SCHEMA_VERSION,
            tool_version: "0.1.0".to_owned(),
            tour_id: "tour-test".to_owned(),
            source: TourSource::codebase(),
            model: "test-model".to_owned(),
            prompt_version: "1".to_owned(),
            redaction_profile: "default".to_owned(),
            position: Position::default(),
            chapters: vec![
                TourChapter {
                    id: "ch-1".to_owned(),
                    title: "On-ramp".to_owned(),
                    stops: vec![
                        stop("stop-1", "src/main.rs", 1, 1),
                        stop("stop-2", "src/main.rs", 2, 2),
                    ],
                },
                TourChapter {
                    id: "ch-2".to_owned(),
                    title: "Core".to_owned(),
                    stops: vec![stop("stop-3", "src/lib.rs", 1, 1)],
                },
            ],
        };
        Session {
            repo_root: temp.path().clone(),
            config: Config::load_from_offline(None, None).expect("offline config"),
            skeleton,
            model,
            tour,
            client: None,
            offline: true,
        }
    }

    #[test]
    fn next_and_prev_move_across_chapters_and_clamp() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "one\ntwo\n");
        temp.write("src/lib.rs", "three\n");
        let mut app = App::new(session(&temp));

        assert_eq!(app.current, 0);
        app.next();
        assert_eq!(app.current, 1);
        app.next();
        assert_eq!(app.current, 2);
        app.next();
        assert_eq!(app.current, 2);
        app.prev();
        assert_eq!(app.current, 1);
        app.prev();
        app.prev();
        assert_eq!(app.current, 0);
    }

    #[test]
    fn outline_selects_a_stop() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "one\ntwo\n");
        temp.write("src/lib.rs", "three\n");
        let mut app = App::new(session(&temp));

        app.toggle_outline();
        assert!(app.outline_open);
        app.outline_next();
        app.outline_select();

        assert!(!app.outline_open);
        assert_eq!(app.current, 1);
        assert_eq!(
            app.current_stop().map(|stop| stop.id.as_str()),
            Some("stop-2")
        );
    }

    #[test]
    fn code_is_loaded_and_scrolled_to_the_stop() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "one\ntwo\nthree\nfour\n");
        temp.write("src/lib.rs", "three\n");
        let mut app = App::new(session(&temp));

        assert_eq!(app.code_lines.len(), 4);
        assert_eq!(app.code_scroll, 0);

        app.next();
        assert_eq!(app.code_scroll, 0);

        app.return_to_current();
        assert_eq!(app.code_scroll, 0);
    }

    #[test]
    fn saved_position_is_restored() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "one\ntwo\n");
        temp.write("src/lib.rs", "three\n");
        let mut session = session(&temp);
        session.tour.position = Position {
            chapter: 1,
            stop: 0,
        };
        let app = App::new(session);

        assert_eq!(app.current, 2);
        assert_eq!(
            app.current_stop().map(|stop| stop.id.as_str()),
            Some("stop-3")
        );
    }

    #[test]
    fn progress_reports_chapter_and_stop() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "one\ntwo\n");
        temp.write("src/lib.rs", "three\n");
        let mut app = App::new(session(&temp));

        assert_eq!(app.progress(), (1, 2, 1, 2));
        app.next();
        assert_eq!(app.progress(), (1, 2, 2, 2));
        app.next();
        assert_eq!(app.progress(), (2, 2, 1, 1));
    }

    #[test]
    fn outline_rows_cover_every_stop() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "one\ntwo\n");
        temp.write("src/lib.rs", "three\n");
        let app = App::new(session(&temp));

        let rows = app.outline_rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].chapter_title, "On-ramp");
        assert_eq!(rows[2].chapter_title, "Core");
        assert_eq!(rows[2].flat_index, 2);
    }

    #[test]
    fn narration_chunks_stream_into_the_current_stop() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "one\ntwo\n");
        temp.write("src/lib.rs", "three\n");
        let mut app = App::new(session(&temp));
        let stop_id = app.current_stop().expect("current stop").id.clone();

        app.handle_narration_event(UiEvent::Chunk {
            stop_id: stop_id.clone(),
            delta: "Hello ".to_owned(),
        });
        app.handle_narration_event(UiEvent::Chunk {
            stop_id: stop_id.clone(),
            delta: "world".to_owned(),
        });
        app.handle_narration_event(UiEvent::Chunk {
            stop_id: "other-stop".to_owned(),
            delta: "ignored".to_owned(),
        });
        assert_eq!(app.explanation, "Hello world");

        let mut stop = app.current_stop().expect("current stop").clone();
        stop.explanation = "Hello world".to_owned();
        stop.status = StopStatus::Explained;
        app.handle_narration_event(UiEvent::StopNarrated {
            stop,
            warnings: Vec::new(),
        });

        assert_eq!(app.status, "Explained");
        assert_eq!(
            app.current_stop().map(|stop| stop.status),
            Some(StopStatus::Explained)
        );
    }
}
