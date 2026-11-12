//! Rendering for the terminal tour.
//!
//! Layout: header progress, a chapter/stop list on the left, the anchored code
//! range on the right, a streaming explanation pane, and a footer of key hints.
//! The outline overlay is drawn on top when open.

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;

/// Draw the whole TUI.
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Percentage(30),
        Constraint::Length(1),
    ])
    .split(area);

    draw_header(frame, chunks[0], app);

    let body = Layout::horizontal([Constraint::Length(32), Constraint::Min(20)]).split(chunks[1]);
    draw_tour_list(frame, body[0], app);
    draw_code(frame, body[1], app);
    draw_explanation(frame, chunks[2], app);
    draw_footer(frame, chunks[3], app);

    if app.outline_open {
        draw_outline(frame, area, app);
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let (chapter, chapter_count, stop, stop_count) = app.progress();
    let progress = if chapter_count == 0 {
        "No stops".to_owned()
    } else {
        format!("Chapter {chapter}/{chapter_count} · Stop {stop}/{stop_count}")
    };
    let columns = Layout::horizontal([Constraint::Min(0), Constraint::Length(6)]).split(area);
    frame.render_widget(
        Paragraph::new(progress).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        columns[0],
    );
    frame.render_widget(
        Paragraph::new("laudit")
            .alignment(Alignment::Right)
            .style(Style::default().fg(Color::DarkGray)),
        columns[1],
    );
}

fn draw_tour_list(frame: &mut Frame, area: Rect, app: &App) {
    let mut items = Vec::new();
    let mut current_row = 0;
    let current = app.current_location();

    for (chapter_index, chapter) in app.tour.chapters.iter().enumerate() {
        items.push(ListItem::new(Line::from(Span::styled(
            format!("Chapter {}: {}", chapter_index + 1, chapter.title),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ))));
        for (stop_index, stop) in chapter.stops.iter().enumerate() {
            let selected = current == Some((chapter_index, stop_index));
            let marker = if selected { ">" } else { " " };
            let title = if stop.title.is_empty() {
                &stop.symbol_id
            } else {
                &stop.title
            };
            let line = Line::from(vec![
                Span::raw(format!(" {marker} ")),
                Span::raw(format!("{}. {title}", stop_index + 1)),
            ]);
            let style = if selected {
                Style::default()
                    .bg(Color::Blue)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            items.push(ListItem::new(line).style(style));
            if selected {
                current_row = items.len() - 1;
            }
        }
    }

    let list = List::new(items).block(Block::bordered().title("Tour"));
    let mut state = ListState::default().with_selected(Some(current_row));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_code(frame: &mut Frame, area: Rect, app: &App) {
    let stop = app.current_stop();
    let title = stop.map_or_else(
        || "Code".to_owned(),
        |stop| format!("{}:{}-{}", stop.file, stop.start_line, stop.end_line),
    );
    let block = Block::bordered().title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.code_lines.is_empty() {
        frame.render_widget(
            Paragraph::new("(no code loaded)").style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return;
    }

    let lines: Vec<Line> = app
        .code_lines
        .iter()
        .enumerate()
        .map(|(index, text)| {
            let line_number = index as u32 + 1;
            let highlighted = stop
                .is_some_and(|stop| line_number >= stop.start_line && line_number <= stop.end_line);
            let style = if highlighted {
                Style::default()
                    .bg(Color::DarkGray)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::from(vec![
                Span::styled(
                    format!("{:>4} ", line_number),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(text.clone(), style),
            ])
        })
        .collect();

    let paragraph = Paragraph::new(lines).scroll((app.code_scroll, 0));
    frame.render_widget(paragraph, inner);
}

fn draw_explanation(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered().title("Explanation");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let mut text = String::new();
    if let Some(stop) = app.current_stop() {
        if !stop.why_here.is_empty() {
            text.push_str("Why: ");
            text.push_str(&stop.why_here);
            text.push_str("\n\n");
        }
    }
    if !app.explanation.is_empty() {
        text.push_str(&app.explanation);
    } else if app.offline {
        text.push_str("(offline: no narration)");
    } else if app.status.is_empty() {
        text.push_str("(no explanation yet)");
    }
    if !app.status.is_empty() && app.status != "Explained" {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&app.status);
    }

    let line_count = wrapped_line_count(&text, inner.width);
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
    let max_scroll =
        u16::try_from(line_count.saturating_sub(inner.height as usize)).unwrap_or(u16::MAX);
    let scroll = if app.follow_explanation {
        max_scroll
    } else {
        app.explanation_scroll.min(max_scroll)
    };
    frame.render_widget(paragraph.scroll((scroll, 0)), inner);
}

fn wrapped_line_count(text: &str, width: u16) -> usize {
    if width == 0 {
        return 0;
    }
    let width = width as usize;
    let mut count = 0;
    for line in text.split('\n') {
        if line.is_empty() {
            count += 1;
            continue;
        }
        let mut current = 0;
        for word in line.split_whitespace() {
            let word_len = word.chars().count();
            if current == 0 {
                current = word_len;
            } else if current + 1 + word_len <= width {
                current += 1 + word_len;
            } else {
                count += 1;
                current = word_len;
            }
            while current > width {
                count += 1;
                current -= width;
            }
        }
        count += 1;
    }
    count
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let hints = if app.outline_open {
        "j/k move  Enter select  Esc close  q quit"
    } else {
        "n/p next/prev  o outline  g return  q quit"
    };
    frame.render_widget(
        Paragraph::new(hints).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn draw_outline(frame: &mut Frame, area: Rect, app: &App) {
    let popup = centered_rect(70, 70, area);
    frame.render_widget(Clear, popup);

    let block = Block::bordered().title("Outline");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let items: Vec<ListItem> = app
        .outline_rows()
        .into_iter()
        .map(|row| {
            ListItem::new(format!(
                "{} · {}  ({}:{})",
                row.chapter_title, row.stop_title, row.file, row.line
            ))
        })
        .collect();

    let list = List::new(items).highlight_style(
        Style::default()
            .bg(Color::Blue)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default().with_selected(Some(app.outline_selected));
    frame.render_stateful_widget(list, inner, &mut state);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(area);
    let horizontal = Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(vertical[1]);
    horizontal[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Session};
    use laudit_core::build;
    use laudit_core::config::Config;
    use laudit_core::model::{
        CodebaseModel, Position, Skeleton, Stop, StopStatus, Tour, TourChapter, TourSource,
        SCHEMA_VERSION,
    };
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("laudit-tui-ui-test-{}-{id}", std::process::id()));
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
            explanation: "The body.".to_owned(),
            refs: Vec::new(),
            file_hash: String::new(),
            status: StopStatus::Explained,
        }
    }

    fn app(temp: &TempDir) -> App {
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
        let model: CodebaseModel = build::offline_model(&skeleton);
        let tour = Tour {
            schema_version: SCHEMA_VERSION,
            tool_version: "0.1.0".to_owned(),
            tour_id: "tour-test".to_owned(),
            source: TourSource::codebase(),
            model: "test-model".to_owned(),
            prompt_version: "1".to_owned(),
            redaction_profile: "default".to_owned(),
            position: Position::default(),
            chapters: vec![TourChapter {
                id: "ch-1".to_owned(),
                title: "On-ramp".to_owned(),
                stops: vec![stop("stop-1", "src/main.rs", 1, 1)],
            }],
        };
        App::new(Session {
            repo_root: temp.path().clone(),
            config: Config::load_from_offline(None, None).expect("offline config"),
            skeleton,
            model,
            tour,
            client: None,
            offline: true,
        })
    }

    fn render(app: &App) -> String {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| draw(frame, app)).expect("draw");
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn renders_header_code_explanation_and_footer() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        let app = app(&temp);

        let rendered = render(&app);

        assert!(rendered.contains("Chapter 1/1"), "got: {rendered}");
        assert!(rendered.contains("Stop 1/1"), "got: {rendered}");
        assert!(rendered.contains("fn main"), "got: {rendered}");
        assert!(rendered.contains("The body."), "got: {rendered}");
        assert!(rendered.contains("n/p next/prev"), "got: {rendered}");
    }

    #[test]
    fn outline_overlay_renders_when_open() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        let mut app = app(&temp);
        app.toggle_outline();

        let rendered = render(&app);

        assert!(rendered.contains("Outline"), "got: {rendered}");
        assert!(rendered.contains("On-ramp"), "got: {rendered}");
        assert!(rendered.contains("Enter select"), "got: {rendered}");
    }
}
