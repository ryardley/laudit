//! Markdown and HTML export of a tour.
//!
//! Export turns a persisted tour artifact into a shareable document (D23). It
//! reads only artifacts and source files — never the LLM — so it works offline
//! and can render tours whose stops are still pending. The documents are
//! deterministic: no timestamps or other run-specific values are written into
//! the body, which keeps golden tests stable.
//!
//! HTML export produces a single self-contained file: inline CSS, inline
//! highlighting, and in-document anchors keyed by symbol ID. No network assets
//! are referenced, so the page opens and works offline.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pulldown_cmark::{html, CowStr, Event, LinkType, Options, Parser, Tag, TagEnd};
use regex::Regex;
use serde::Serialize;
use syntect::html::{ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

use crate::build;
use crate::model::{CodebaseModel, Skeleton, Stop, Tour};
use crate::plan::{self, PlanError};
use crate::recon::Language;
use crate::status;

/// Fallback document title when the repository has no usable directory name.
pub const DEFAULT_TITLE: &str = "laudit tour";

/// Options controlling markdown rendering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarkdownOptions {
    /// Document title. Defaults to the repository directory name.
    pub title: Option<String>,
}

/// A summary of a markdown export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportReport {
    /// Tour identifier that was rendered.
    pub tour_id: String,
    /// Path of the tour artifact.
    pub tour_path: PathBuf,
    /// Path the markdown was written to, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_path: Option<PathBuf>,
    /// Number of chapters rendered.
    pub chapters: usize,
    /// Number of stops rendered.
    pub stops: usize,
    /// The rendered markdown.
    #[serde(skip)]
    pub markdown: String,
}

/// Options controlling HTML rendering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HtmlOptions {
    /// Document title. Defaults to the repository directory name.
    pub title: Option<String>,
}

/// A summary of an HTML export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HtmlExportReport {
    /// Tour identifier that was rendered.
    pub tour_id: String,
    /// Path of the tour artifact.
    pub tour_path: PathBuf,
    /// Path the HTML was written to, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_path: Option<PathBuf>,
    /// Number of chapters rendered.
    pub chapters: usize,
    /// Number of stops rendered.
    pub stops: usize,
    /// The rendered HTML.
    #[serde(skip)]
    pub html: String,
}

/// Render a tour as a markdown document.
///
/// The document title defaults to the repository directory name. Every stop is
/// rendered as a heading with its `file:line` range, a fenced code block of the
/// exact source range, the `why_here` line, the explanation, and a `Refs:` line
/// linking each referenced symbol to its definition.
pub fn render_markdown(
    repo_root: impl AsRef<Path>,
    tour: &Tour,
    model: &CodebaseModel,
    skeleton: Option<&Skeleton>,
    options: &MarkdownOptions,
) -> Result<String, ExportError> {
    let repo_root = repo_root.as_ref();
    let title = options
        .title
        .clone()
        .unwrap_or_else(|| default_title(repo_root, model));
    let symbols = symbol_index(tour, skeleton);

    let mut out = String::new();
    let _ = writeln!(out, "# {title}");

    for chapter in &tour.chapters {
        out.push('\n');
        let _ = writeln!(out, "## {}", chapter.title);

        for stop in &chapter.stops {
            out.push('\n');
            let stop_title = if stop.title.is_empty() {
                &stop.symbol_id
            } else {
                &stop.title
            };
            let _ = writeln!(
                out,
                "### {stop_title} — {}:{}-{}",
                stop.file, stop.start_line, stop.end_line
            );
            out.push('\n');

            let code = read_range(repo_root, stop)?;
            let language = Language::from_path(Path::new(&stop.file)).map_or("", Language::as_str);
            let fence = fence_for(&code);
            let _ = writeln!(out, "{fence}{language}");
            out.push_str(&code);
            if !code.ends_with('\n') {
                out.push('\n');
            }
            let _ = writeln!(out, "{fence}");

            if !stop.why_here.is_empty() {
                out.push('\n');
                let _ = writeln!(out, "**Why here:** {}", stop.why_here);
            }

            if !stop.explanation.is_empty() {
                out.push('\n');
                out.push_str(stop.explanation.trim_end());
                out.push('\n');
            }

            if !stop.refs.is_empty() {
                out.push('\n');
                let refs: Vec<String> = stop
                    .refs
                    .iter()
                    .map(|id| render_ref(id, &symbols))
                    .collect();
                let _ = writeln!(out, "Refs: {}", refs.join(", "));
            }
        }
    }

    Ok(out)
}

/// Render a tour as a single self-contained HTML document.
///
/// The page has a chapter navigation sidebar, one section per stop with a code
/// pane and a prose pane, and in-document anchors keyed by symbol ID. Code is
/// highlighted with syntect and prose is rendered from markdown. All CSS and
/// JavaScript are inline; the document references no network assets.
pub fn render_html(
    repo_root: impl AsRef<Path>,
    tour: &Tour,
    model: &CodebaseModel,
    skeleton: Option<&Skeleton>,
    options: &HtmlOptions,
) -> Result<String, ExportError> {
    let repo_root = repo_root.as_ref();
    let title = options
        .title
        .clone()
        .unwrap_or_else(|| default_title(repo_root, model));
    let symbols = symbol_index(tour, skeleton);
    let known_symbols: BTreeSet<String> = symbols.keys().cloned().collect();
    let stop_symbols: BTreeSet<String> = tour
        .chapters
        .iter()
        .flat_map(|chapter| chapter.stops.iter().map(|stop| stop.symbol_id.clone()))
        .collect();
    let stop_count: usize = tour
        .chapters
        .iter()
        .map(|chapter| chapter.stops.len())
        .sum();

    let mut out = String::new();
    out.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
    out.push_str("<meta charset=\"utf-8\">\n");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    let _ = writeln!(out, "<title>{}</title>", escape_html(&title));
    out.push_str("<style>\n");
    out.push_str(HTML_CSS);
    out.push_str("</style>\n</head>\n<body>\n");
    let _ = writeln!(
        out,
        "<header class=\"topbar\"><h1>{}</h1><p class=\"tour-meta\">{} chapters · {} stops</p></header>",
        escape_html(&title),
        tour.chapters.len(),
        stop_count
    );
    out.push_str("<div class=\"layout\">\n");
    render_nav(&mut out, tour);
    out.push_str("<main class=\"content\">\n");

    for chapter in &tour.chapters {
        let _ = writeln!(
            out,
            "<section class=\"chapter\" id=\"{}\">",
            chapter_anchor(&chapter.id)
        );
        let _ = writeln!(out, "<h2>{}</h2>", escape_html(&chapter.title));
        for stop in &chapter.stops {
            render_stop(&mut out, repo_root, stop, &symbols, &known_symbols)?;
        }
        out.push_str("</section>\n");
    }

    out.push_str("<section class=\"symbol-index\" hidden aria-hidden=\"true\">\n");
    for id in symbols.keys() {
        if !stop_symbols.contains(id) {
            let _ = writeln!(out, "<a id=\"{}\"></a>", symbol_anchor(id));
        }
    }
    out.push_str("</section>\n</main>\n</div>\n");
    out.push_str("<script>\n");
    out.push_str(HTML_JS);
    out.push_str("</script>\n</body>\n</html>\n");
    Ok(out)
}

/// Load the latest (or named) tour and its artifacts, render HTML, and
/// optionally write it to `out`.
///
/// When `out` is a directory, ends with a path separator, or is a new path
/// without an extension, the document is written to `index.html` inside it.
/// Otherwise `out` is treated as a file. When `out` is `None` the HTML is
/// returned in [`HtmlExportReport::html`] for the caller to print. No LLM or
/// network access is involved.
pub fn export_html(
    repo_root: impl AsRef<Path>,
    tour_id: Option<&str>,
    out: Option<&Path>,
) -> Result<HtmlExportReport, ExportError> {
    let repo_root = repo_root.as_ref();
    let (tour, tour_path) = load_tour(repo_root, tour_id)?;
    let skeleton = load_skeleton(repo_root)?;
    let model = load_model(repo_root, skeleton.as_ref())?;
    let html = render_html(
        repo_root,
        &tour,
        &model,
        skeleton.as_ref(),
        &HtmlOptions::default(),
    )?;

    let output_path = match out {
        Some(path) => Some(write_output(&resolve_html_output(path), &html)?),
        None => None,
    };

    let chapters = tour.chapters.len();
    let stops = tour
        .chapters
        .iter()
        .map(|chapter| chapter.stops.len())
        .sum();
    Ok(HtmlExportReport {
        tour_id: tour.tour_id,
        tour_path,
        output_path,
        chapters,
        stops,
        html,
    })
}

/// Load the latest (or named) tour and its artifacts, render markdown, and
/// optionally write it to `out`.
///
/// When `out` is `None` the markdown is returned in [`ExportReport::markdown`]
/// for the caller to print. No LLM or network access is involved.
pub fn export_markdown(
    repo_root: impl AsRef<Path>,
    tour_id: Option<&str>,
    out: Option<&Path>,
) -> Result<ExportReport, ExportError> {
    let repo_root = repo_root.as_ref();
    let (tour, tour_path) = load_tour(repo_root, tour_id)?;
    let skeleton = load_skeleton(repo_root)?;
    let model = load_model(repo_root, skeleton.as_ref())?;
    let markdown = render_markdown(
        repo_root,
        &tour,
        &model,
        skeleton.as_ref(),
        &MarkdownOptions::default(),
    )?;

    let output_path = match out {
        Some(path) => Some(write_output(path, &markdown)?),
        None => None,
    };

    let chapters = tour.chapters.len();
    let stops = tour
        .chapters
        .iter()
        .map(|chapter| chapter.stops.len())
        .sum();
    Ok(ExportReport {
        tour_id: tour.tour_id,
        tour_path,
        output_path,
        chapters,
        stops,
        markdown,
    })
}

fn load_tour(repo_root: &Path, tour_id: Option<&str>) -> Result<(Tour, PathBuf), ExportError> {
    let tour_id = match tour_id {
        Some(tour_id) => tour_id.to_owned(),
        None => status::latest_tour_id(repo_root)
            .map_err(|source| ExportError::Io {
                path: repo_root.join(plan::TOURS_DIR),
                source,
            })?
            .ok_or(ExportError::NoTour)?,
    };
    let tour = plan::load_tour(repo_root, &tour_id)?;
    let path = plan::tour_path(repo_root, &tour_id);
    Ok((tour, path))
}

fn load_skeleton(repo_root: &Path) -> Result<Option<Skeleton>, ExportError> {
    match plan::load_skeleton(repo_root) {
        Ok(skeleton) => Ok(Some(skeleton)),
        Err(PlanError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ExportError::Plan(error)),
    }
}

fn load_model(repo_root: &Path, skeleton: Option<&Skeleton>) -> Result<CodebaseModel, ExportError> {
    match plan::load_model(repo_root) {
        Ok(model) => Ok(model),
        Err(PlanError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            match skeleton {
                Some(skeleton) => Ok(build::offline_model(skeleton)),
                None => Err(ExportError::NoModel),
            }
        }
        Err(error) => Err(ExportError::Plan(error)),
    }
}

fn default_title(repo_root: &Path, model: &CodebaseModel) -> String {
    let canonical = fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
    if let Some(name) = canonical.file_name().and_then(|name| name.to_str()) {
        if !name.is_empty() {
            return name.to_owned();
        }
    }
    if let Some(subsystem) = model.subsystems.first() {
        if !subsystem.title.is_empty() {
            return subsystem.title.clone();
        }
    }
    DEFAULT_TITLE.to_owned()
}

fn write_output(path: &Path, contents: &str) -> Result<PathBuf, ExportError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|source| ExportError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
    }
    fs::write(path, contents).map_err(|source| ExportError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(path.to_path_buf())
}

fn resolve_html_output(path: &Path) -> PathBuf {
    let looks_like_directory = path.is_dir()
        || path_ends_with_separator(path)
        || (!path.exists() && path.extension().is_none());
    if looks_like_directory {
        path.join("index.html")
    } else {
        path.to_path_buf()
    }
}

fn path_ends_with_separator(path: &Path) -> bool {
    let rendered = path.to_string_lossy();
    rendered.ends_with('/') || rendered.ends_with('\\')
}

fn render_nav(out: &mut String, tour: &Tour) {
    out.push_str("<nav class=\"sidebar\" aria-label=\"Tour navigation\">\n");
    for chapter in &tour.chapters {
        out.push_str("<section class=\"nav-chapter\">\n");
        let _ = writeln!(
            out,
            "<a class=\"nav-chapter-title\" href=\"#{}\">{}</a>",
            chapter_anchor(&chapter.id),
            escape_html(&chapter.title)
        );
        out.push_str("<ol class=\"nav-stops\">\n");
        for stop in &chapter.stops {
            let _ = writeln!(
                out,
                "<li><a href=\"#{}\">{}</a></li>",
                stop_anchor(&stop.id),
                escape_html(stop_title(stop))
            );
        }
        out.push_str("</ol>\n</section>\n");
    }
    out.push_str("</nav>\n");
}

fn render_stop(
    out: &mut String,
    repo_root: &Path,
    stop: &Stop,
    symbols: &BTreeMap<String, SymbolRef>,
    known_symbols: &BTreeSet<String>,
) -> Result<(), ExportError> {
    let _ = writeln!(
        out,
        "<article class=\"stop\" id=\"{}\">",
        stop_anchor(&stop.id)
    );
    let _ = writeln!(
        out,
        "<a class=\"symbol-anchor\" id=\"{}\"></a>",
        symbol_anchor(&stop.symbol_id)
    );
    let _ = writeln!(
        out,
        "<header class=\"stop-header\"><h3>{}</h3><p class=\"location\">{}:{}-{}</p></header>",
        escape_html(stop_title(stop)),
        escape_html(&stop.file),
        stop.start_line,
        stop.end_line
    );
    out.push_str("<div class=\"panes\">\n<div class=\"code-pane\">\n");

    let code = read_range(repo_root, stop)?;
    let language = Language::from_path(Path::new(&stop.file));
    let highlighted = highlight_code(&code, language);
    let language_class = language.map_or("", Language::as_str);
    let _ = writeln!(
        out,
        "<pre><code class=\"language-{}\">{}</code></pre>",
        escape_html(language_class),
        highlighted
    );
    out.push_str("</div>\n<div class=\"prose-pane\">\n");

    if !stop.why_here.is_empty() {
        let _ = writeln!(
            out,
            "<p class=\"why\"><strong>Why here:</strong> {}</p>",
            escape_html(&stop.why_here)
        );
    }

    if !stop.explanation.is_empty() {
        out.push_str("<div class=\"explanation\">\n");
        out.push_str(&render_markdown_body(&stop.explanation, known_symbols));
        out.push_str("</div>\n");
    }

    if !stop.refs.is_empty() {
        out.push_str("<div class=\"refs\"><span class=\"refs-label\">Refs:</span> ");
        for (index, id) in stop.refs.iter().enumerate() {
            if index > 0 {
                out.push_str(", ");
            }
            match symbols.get(id) {
                Some(symbol) => {
                    let _ = write!(
                        out,
                        "<a href=\"#{}\">{}</a>",
                        symbol_anchor(id),
                        escape_html(&symbol.name)
                    );
                }
                None => out.push_str(&escape_html(id)),
            }
        }
        out.push_str("</div>\n");
    }

    out.push_str("</div>\n</div>\n</article>\n");
    Ok(())
}

fn stop_title(stop: &Stop) -> &str {
    if stop.title.is_empty() {
        &stop.symbol_id
    } else {
        &stop.title
    }
}

fn highlight_code(code: &str, language: Option<Language>) -> String {
    let syntax_set = syntax_set();
    let syntax = language
        .and_then(|language| syntax_for(language, syntax_set))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());
    let mut generator = ClassedHTMLGenerator::new_with_class_style(
        syntax,
        syntax_set,
        ClassStyle::SpacedPrefixed { prefix: "hl-" },
    );
    let mut code = code.to_owned();
    if !code.ends_with('\n') {
        code.push('\n');
    }
    for line in LinesWithEndings::from(&code) {
        if generator
            .parse_html_for_line_which_includes_newline(line)
            .is_err()
        {
            return escape_html(&code);
        }
    }
    generator.finalize()
}

fn syntax_set() -> &'static SyntaxSet {
    static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAX_SET.get_or_init(two_face::syntax::extra_newlines)
}

fn syntax_for(language: Language, syntax_set: &SyntaxSet) -> Option<&SyntaxReference> {
    let extension = match language {
        Language::Rust => "rs",
        Language::TypeScript => "ts",
        Language::Tsx => "tsx",
        Language::JavaScript => "js",
        Language::Bash => "sh",
        Language::Toml => "toml",
        Language::Yaml => "yaml",
        Language::Json => "json",
        Language::Markdown => "md",
        Language::Sql => "sql",
    };
    syntax_set
        .find_syntax_by_extension(extension)
        .or_else(|| syntax_set.find_syntax_by_token(extension))
        .or_else(|| match language {
            Language::Tsx => syntax_set.find_syntax_by_extension("ts"),
            Language::TypeScript => syntax_set.find_syntax_by_extension("js"),
            _ => None,
        })
}

fn render_markdown_body(markdown: &str, known_symbols: &BTreeSet<String>) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);
    let parser = Parser::new_ext(markdown, options);
    let mut events = Vec::new();
    let mut in_code_block = false;
    let mut link_depth = 0usize;
    for event in parser {
        match event {
            Event::Start(Tag::CodeBlock(_)) => {
                in_code_block = true;
                events.push(event);
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                events.push(event);
            }
            Event::Start(Tag::Link { .. }) => {
                link_depth += 1;
                events.push(event);
            }
            Event::End(TagEnd::Link) => {
                link_depth = link_depth.saturating_sub(1);
                events.push(event);
            }
            Event::Text(text) if !in_code_block && link_depth == 0 => {
                push_linked_text(&mut events, &text, known_symbols);
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                events.push(Event::Text(html));
            }
            other => events.push(other),
        }
    }
    let mut html = String::new();
    html::push_html(&mut html, events.into_iter());
    html
}

fn push_linked_text<'a>(events: &mut Vec<Event<'a>>, text: &str, known_symbols: &BTreeSet<String>) {
    let mut last = 0;
    for found in symbol_ref_regex().find_iter(text) {
        if found.start() > last {
            events.push(Event::Text(CowStr::from(
                text[last..found.start()].to_owned(),
            )));
        }
        let id = found.as_str();
        if known_symbols.contains(id) {
            events.push(Event::Start(Tag::Link {
                link_type: LinkType::Inline,
                dest_url: CowStr::from(format!("#{}", symbol_anchor(id))),
                title: CowStr::from(String::new()),
                id: CowStr::from(String::new()),
            }));
            events.push(Event::Text(CowStr::from(id.to_owned())));
            events.push(Event::End(TagEnd::Link));
        } else {
            events.push(Event::Text(CowStr::from(id.to_owned())));
        }
        last = found.end();
    }
    if last < text.len() {
        events.push(Event::Text(CowStr::from(text[last..].to_owned())));
    }
}

fn symbol_ref_regex() -> &'static Regex {
    static SYMBOL_REF: OnceLock<Regex> = OnceLock::new();
    SYMBOL_REF
        .get_or_init(|| Regex::new(r"\bsym-[A-Za-z0-9_-]+\b").expect("symbol ref regex compiles"))
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn anchor(prefix: &str, id: &str) -> String {
    let mut anchor = String::with_capacity(prefix.len() + id.len());
    anchor.push_str(prefix);
    for character in id.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':') {
            anchor.push(character);
        } else {
            anchor.push('-');
        }
    }
    anchor
}

fn chapter_anchor(id: &str) -> String {
    anchor("chapter-", id)
}

fn stop_anchor(id: &str) -> String {
    anchor("stop-", id)
}

fn symbol_anchor(id: &str) -> String {
    anchor("symbol-", id)
}

const HTML_CSS: &str = r#"
:root {
  color-scheme: light dark;
  --bg: #ffffff;
  --fg: #1f2328;
  --muted: #656d76;
  --border: #d0d7de;
  --accent: #0969da;
  --code-bg: #f6f8fa;
  --sidebar-bg: #f6f8fa;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #0d1117;
    --fg: #e6edf3;
    --muted: #8b949e;
    --border: #30363d;
    --accent: #58a6ff;
    --code-bg: #161b22;
    --sidebar-bg: #161b22;
  }
}
* { box-sizing: border-box; }
html { scroll-behavior: smooth; }
body {
  margin: 0;
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif;
  background: var(--bg);
  color: var(--fg);
  line-height: 1.5;
}
a { color: var(--accent); }
.topbar { padding: 1rem 1.5rem; border-bottom: 1px solid var(--border); }
.topbar h1 { margin: 0; font-size: 1.4rem; }
.tour-meta { margin: .25rem 0 0; color: var(--muted); font-size: .9rem; }
.layout { display: grid; grid-template-columns: 18rem minmax(0, 1fr); min-height: calc(100vh - 5rem); }
.sidebar {
  position: sticky;
  top: 0;
  align-self: start;
  max-height: 100vh;
  overflow: auto;
  padding: 1rem;
  border-right: 1px solid var(--border);
  background: var(--sidebar-bg);
}
.nav-chapter { margin-bottom: 1rem; }
.nav-chapter-title { font-weight: 600; color: var(--fg); text-decoration: none; }
.nav-stops { margin: .25rem 0 0; padding-left: 1.1rem; }
.nav-stops a { color: var(--muted); text-decoration: none; font-size: .9rem; }
.nav-stops a:hover, .nav-stops a.active { color: var(--accent); }
.content { padding: 1.5rem; max-width: 90rem; }
.chapter { margin-bottom: 2.5rem; }
.chapter h2 { border-bottom: 1px solid var(--border); padding-bottom: .25rem; }
.stop { margin: 1.5rem 0; border: 1px solid var(--border); border-radius: .5rem; overflow: hidden; }
.stop-header { padding: .75rem 1rem; border-bottom: 1px solid var(--border); background: var(--sidebar-bg); }
.stop-header h3 { margin: 0; font-size: 1.1rem; }
.location {
  margin: .25rem 0 0;
  color: var(--muted);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: .85rem;
}
.panes { display: grid; grid-template-columns: minmax(0, 1.1fr) minmax(0, 1fr); }
.code-pane { border-right: 1px solid var(--border); background: var(--code-bg); overflow: auto; }
.code-pane pre { margin: 0; padding: 1rem; overflow: auto; }
.code-pane code {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: .85rem;
  line-height: 1.45;
}
.prose-pane { padding: 1rem; }
.prose-pane > :first-child { margin-top: 0; }
.why { color: var(--muted); }
.refs { margin-top: 1rem; padding-top: .75rem; border-top: 1px solid var(--border); font-size: .9rem; }
.refs-label { font-weight: 600; }
.symbol-anchor { display: block; position: relative; top: -1rem; visibility: hidden; }
.hl-keyword, .hl-storage { color: #cf222e; }
.hl-string { color: #0a3069; }
.hl-comment { color: #6e7781; font-style: italic; }
.hl-constant, .hl-support { color: #0550ae; }
.hl-entity, .hl-variable { color: #953800; }
.hl-meta { color: #8250df; }
.hl-invalid { color: #ffffff; background: #cf222e; }
@media (prefers-color-scheme: dark) {
  .hl-keyword, .hl-storage { color: #ff7b72; }
  .hl-string { color: #a5d6ff; }
  .hl-comment { color: #8b949e; }
  .hl-constant, .hl-support { color: #79c0ff; }
  .hl-entity, .hl-variable { color: #ffa657; }
  .hl-meta { color: #d2a8ff; }
}
@media (max-width: 60rem) {
  .layout { grid-template-columns: 1fr; }
  .sidebar { position: static; max-height: none; border-right: 0; border-bottom: 1px solid var(--border); }
  .panes { grid-template-columns: 1fr; }
  .code-pane { border-right: 0; border-bottom: 1px solid var(--border); }
}
"#;

const HTML_JS: &str = r#"
(function () {
  var links = Array.prototype.slice.call(document.querySelectorAll('.nav-stops a'));
  var byId = {};
  links.forEach(function (link) {
    byId[link.getAttribute('href').slice(1)] = link;
  });
  function activate() {
    links.forEach(function (link) { link.classList.remove('active'); });
    var link = byId[location.hash.slice(1)];
    if (link) { link.classList.add('active'); }
  }
  window.addEventListener('hashchange', activate);
  activate();
})();
"#;

fn read_range(repo_root: &Path, stop: &Stop) -> Result<String, ExportError> {
    let path = repo_root.join(&stop.file);
    let bytes = fs::read(&path).map_err(|source| ExportError::Io {
        path: path.clone(),
        source,
    })?;
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let start = stop.start_line.max(1) as usize;
    let end = stop.end_line.max(stop.start_line) as usize;
    if start > lines.len() || end < start {
        return Ok(String::new());
    }
    let end = end.min(lines.len());
    let mut code = lines[start - 1..end].join("\n");
    code.push('\n');
    Ok(code)
}

struct SymbolRef {
    name: String,
    file: String,
    line: u32,
}

fn symbol_index(tour: &Tour, skeleton: Option<&Skeleton>) -> BTreeMap<String, SymbolRef> {
    let mut index = BTreeMap::new();
    if let Some(skeleton) = skeleton {
        for symbol in &skeleton.symbols {
            index.insert(
                symbol.id.clone(),
                SymbolRef {
                    name: symbol.name.clone(),
                    file: symbol.file.to_string_lossy().into_owned(),
                    line: symbol.line,
                },
            );
        }
    }
    for chapter in &tour.chapters {
        for stop in &chapter.stops {
            index
                .entry(stop.symbol_id.clone())
                .or_insert_with(|| SymbolRef {
                    name: if stop.title.is_empty() {
                        stop.symbol_id.clone()
                    } else {
                        stop.title.clone()
                    },
                    file: stop.file.clone(),
                    line: stop.start_line,
                });
        }
    }
    index
}

fn render_ref(id: &str, symbols: &BTreeMap<String, SymbolRef>) -> String {
    match symbols.get(id) {
        Some(symbol) => format!(
            "[{} ({}:{})]({}:{})",
            symbol.name, symbol.file, symbol.line, symbol.file, symbol.line
        ),
        None => id.to_owned(),
    }
}

fn fence_for(code: &str) -> String {
    let mut longest = 0;
    let mut current = 0;
    for character in code.chars() {
        if character == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

/// Errors produced while exporting a tour.
#[derive(Debug)]
pub enum ExportError {
    /// An artifact or source file could not be read or written.
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        source: io::Error,
    },
    /// A planning or artifact error.
    Plan(PlanError),
    /// No tour artifact exists to export.
    NoTour,
    /// Neither a model artifact nor a skeleton is available.
    NoModel,
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "failed to access {}: {source}", path.display())
            }
            Self::Plan(error) => write!(f, "{error}"),
            Self::NoTour => write!(
                f,
                "no tour artifact found; run `laudit build` or `laudit tour` first"
            ),
            Self::NoModel => write!(f, "no model artifact found; run `laudit build` first"),
        }
    }
}

impl std::error::Error for ExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Plan(error) => Some(error),
            Self::NoTour | Self::NoModel => None,
        }
    }
}

impl From<PlanError> for ExportError {
    fn from(error: PlanError) -> Self {
        Self::Plan(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Position, StopStatus, TourChapter, TourSource, SCHEMA_VERSION};
    use crate::recon::{Community, Entrypoint, EntrypointKind, FileNode, SymbolNode};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("laudit-export-test-{}-{id}", std::process::id()));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
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

    fn symbol(id: &str, file: &str, name: &str, line: u32) -> SymbolNode {
        SymbolNode {
            id: id.to_owned(),
            file: PathBuf::from(file),
            name: name.to_owned(),
            line,
            rank: 1.0,
            community: 0,
        }
    }

    fn skeleton(symbols: Vec<SymbolNode>) -> Skeleton {
        Skeleton {
            schema_version: SCHEMA_VERSION,
            tool_version: "0.1.0".to_owned(),
            repo_id: "fnv1a128:abc".to_owned(),
            head: None,
            dirty_hash: "fnv1a128:def".to_owned(),
            files: vec![FileNode {
                path: PathBuf::from("src/main.rs"),
                rank: 1.0,
                community: 0,
            }],
            symbols,
            communities: vec![Community {
                id: 0,
                files: vec![PathBuf::from("src/main.rs")],
                symbols: Vec::new(),
            }],
            entrypoints: vec![Entrypoint {
                file: PathBuf::from("src/main.rs"),
                line: 1,
                kind: EntrypointKind::RustMain,
                reason: "Rust binary crate root (main.rs)".to_owned(),
                score: 100,
            }],
            ranked_files: vec!["src/main.rs".to_owned()],
            ranked_symbols: Vec::new(),
            provenance: BTreeMap::new(),
        }
    }

    fn model() -> CodebaseModel {
        build::offline_model(&skeleton(Vec::new()))
    }

    fn stop(id: &str, file: &str, start: u32, end: u32, refs: Vec<&str>) -> Stop {
        Stop {
            id: id.to_owned(),
            file: file.to_owned(),
            start_line: start,
            end_line: end,
            symbol_id: format!("sym-{id}"),
            title: id.to_owned(),
            why_here: "It is part of the tour.".to_owned(),
            explanation: "The explanation.".to_owned(),
            refs: refs.into_iter().map(str::to_owned).collect(),
            file_hash: String::new(),
            status: StopStatus::Explained,
        }
    }

    fn tour(stops: Vec<Stop>) -> Tour {
        Tour {
            schema_version: SCHEMA_VERSION,
            tool_version: "0.1.0".to_owned(),
            tour_id: "tour-test".to_owned(),
            source: TourSource::codebase(),
            model: "test-model".to_owned(),
            prompt_version: "1".to_owned(),
            redaction_profile: "default".to_owned(),
            position: Position::default(),
            chapters: vec![TourChapter {
                id: "on-ramp".to_owned(),
                title: "On-ramp".to_owned(),
                stops,
            }],
        }
    }

    #[test]
    fn renders_headings_code_why_and_refs() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {\n    greet();\n}\n");
        temp.write("src/lib.rs", "pub fn greet() {}\n");
        let skeleton = skeleton(vec![
            symbol("sym-main", "src/main.rs", "main", 1),
            symbol("sym-greet", "src/lib.rs", "greet", 1),
        ]);
        let tour = tour(vec![stop("main", "src/main.rs", 1, 3, vec!["sym-greet"])]);

        let markdown = render_markdown(
            temp.path(),
            &tour,
            &model(),
            Some(&skeleton),
            &MarkdownOptions::default(),
        )
        .expect("render");

        assert!(markdown.starts_with("# "));
        assert!(markdown.contains("## On-ramp"));
        assert!(markdown.contains("### main — src/main.rs:1-3"));
        assert!(markdown.contains("```rust\nfn main() {\n    greet();\n}\n```"));
        assert!(markdown.contains("**Why here:** It is part of the tour."));
        assert!(markdown.contains("The explanation."));
        assert!(markdown.contains("Refs: [greet (src/lib.rs:1)](src/lib.rs:1)"));
    }

    #[test]
    fn code_fence_uses_the_file_extension_language() {
        let temp = TempDir::new();
        temp.write("config/app.toml", "[server]\nport = 8080\n");
        let skeleton = skeleton(vec![symbol("sym-server", "config/app.toml", "server", 1)]);
        let tour = tour(vec![stop("server", "config/app.toml", 1, 2, Vec::new())]);

        let markdown = render_markdown(
            temp.path(),
            &tour,
            &model(),
            Some(&skeleton),
            &MarkdownOptions::default(),
        )
        .expect("render");

        assert!(markdown.contains("```toml\n[server]\nport = 8080\n```"));
    }

    #[test]
    fn empty_prose_and_refs_are_omitted() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        let skeleton = skeleton(vec![symbol("sym-main", "src/main.rs", "main", 1)]);
        let mut stop = stop("main", "src/main.rs", 1, 1, Vec::new());
        stop.why_here.clear();
        stop.explanation.clear();
        let tour = tour(vec![stop]);

        let markdown = render_markdown(
            temp.path(),
            &tour,
            &model(),
            Some(&skeleton),
            &MarkdownOptions::default(),
        )
        .expect("render");

        assert!(!markdown.contains("**Why here:**"));
        assert!(!markdown.contains("Refs:"));
        assert!(markdown.contains("```rust\nfn main() {}\n```"));
    }

    #[test]
    fn explicit_title_overrides_the_repository_name() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        let skeleton = skeleton(vec![symbol("sym-main", "src/main.rs", "main", 1)]);
        let tour = tour(vec![stop("main", "src/main.rs", 1, 1, Vec::new())]);

        let markdown = render_markdown(
            temp.path(),
            &tour,
            &model(),
            Some(&skeleton),
            &MarkdownOptions {
                title: Some("Custom title".to_owned()),
            },
        )
        .expect("render");

        assert!(markdown.starts_with("# Custom title\n"));
    }

    #[test]
    fn renders_without_a_skeleton_using_tour_stops_for_refs() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        temp.write("src/lib.rs", "pub fn greet() {}\n");
        let tour = tour(vec![
            stop("main", "src/main.rs", 1, 1, vec!["sym-greet"]),
            stop("greet", "src/lib.rs", 1, 1, Vec::new()),
        ]);

        let markdown = render_markdown(
            temp.path(),
            &tour,
            &model(),
            None,
            &MarkdownOptions::default(),
        )
        .expect("render");

        assert!(markdown.contains("### main — src/main.rs:1-1"));
        assert!(markdown.contains("Refs: [greet (src/lib.rs:1)](src/lib.rs:1)"));
    }

    #[test]
    fn missing_source_file_is_an_error() {
        let temp = TempDir::new();
        let skeleton = skeleton(vec![symbol("sym-main", "src/main.rs", "main", 1)]);
        let tour = tour(vec![stop("main", "src/main.rs", 1, 1, Vec::new())]);

        let error = render_markdown(
            temp.path(),
            &tour,
            &model(),
            Some(&skeleton),
            &MarkdownOptions::default(),
        )
        .expect_err("missing source");

        assert!(matches!(error, ExportError::Io { .. }));
    }

    #[test]
    fn export_writes_to_the_requested_path() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        let skeleton = skeleton(vec![symbol("sym-main", "src/main.rs", "main", 1)]);
        let tour = tour(vec![stop("main", "src/main.rs", 1, 1, Vec::new())]);
        plan::write_tour(temp.path(), &tour).expect("write tour");
        build::write_skeleton(temp.path(), &skeleton).expect("write skeleton");
        build::write_model(temp.path(), &model()).expect("write model");

        let out = temp.path().join("out/tour.md");
        let report = export_markdown(temp.path(), None, Some(&out)).expect("export");

        assert_eq!(report.tour_id, "tour-test");
        assert_eq!(report.chapters, 1);
        assert_eq!(report.stops, 1);
        assert_eq!(report.output_path.as_deref(), Some(out.as_path()));
        assert_eq!(
            fs::read_to_string(&out).expect("read output"),
            report.markdown
        );
    }

    #[test]
    fn export_without_a_tour_is_an_error() {
        let temp = TempDir::new();
        let error = export_markdown(temp.path(), None, None).expect_err("no tour");
        assert!(matches!(error, ExportError::NoTour));
    }

    #[test]
    fn renders_self_contained_html_with_navigation_code_and_prose() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {\n    greet();\n}\n");
        temp.write("src/lib.rs", "pub fn greet() {}\n");
        let skeleton = skeleton(vec![
            symbol("sym-main", "src/main.rs", "main", 1),
            symbol("sym-greet", "src/lib.rs", "greet", 1),
        ]);
        let tour = tour(vec![stop("main", "src/main.rs", 1, 3, vec!["sym-greet"])]);

        let html = render_html(
            temp.path(),
            &tour,
            &model(),
            Some(&skeleton),
            &HtmlOptions::default(),
        )
        .expect("render");

        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("<title>"));
        assert!(html.contains("class=\"sidebar\""));
        assert!(html.contains("id=\"chapter-on-ramp\""));
        assert!(html.contains("id=\"stop-main\""));
        assert!(html.contains("id=\"symbol-sym-main\""));
        assert!(html.contains("href=\"#symbol-sym-greet\""));
        assert!(html.contains("<pre><code class=\"language-rust\">"));
        assert!(html.contains("hl-keyword"));
        assert!(html.contains("The explanation."));
        assert!(!html.contains("<link "));
        assert!(!html.contains("src=\"http"));
    }

    #[test]
    fn html_escapes_code_and_raw_html_in_prose() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() { let x = \"<tag>\"; }\n");
        let skeleton = skeleton(vec![symbol("sym-main", "src/main.rs", "main", 1)]);
        let mut stop = stop("main", "src/main.rs", 1, 1, Vec::new());
        stop.explanation = "Use `<tag>` and <script>alert(1)</script>.".to_owned();
        let tour = tour(vec![stop]);

        let html = render_html(
            temp.path(),
            &tour,
            &model(),
            Some(&skeleton),
            &HtmlOptions::default(),
        )
        .expect("render");

        assert!(html.contains("&lt;tag&gt;"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains("<script>alert(1)</script>"));
    }

    #[test]
    fn html_export_writes_a_file_or_a_directory_index() {
        let temp = TempDir::new();
        temp.write("src/main.rs", "fn main() {}\n");
        let skeleton = skeleton(vec![symbol("sym-main", "src/main.rs", "main", 1)]);
        let tour = tour(vec![stop("main", "src/main.rs", 1, 1, Vec::new())]);
        plan::write_tour(temp.path(), &tour).expect("write tour");
        build::write_skeleton(temp.path(), &skeleton).expect("write skeleton");
        build::write_model(temp.path(), &model()).expect("write model");

        let file = temp.path().join("out/tour.html");
        let report = export_html(temp.path(), None, Some(&file)).expect("export file");
        assert_eq!(report.output_path.as_deref(), Some(file.as_path()));
        assert_eq!(fs::read_to_string(&file).expect("read file"), report.html);

        let dir = temp.path().join("site");
        fs::create_dir_all(&dir).expect("create dir");
        let report = export_html(temp.path(), None, Some(&dir)).expect("export dir");
        let index = dir.join("index.html");
        assert_eq!(report.output_path.as_deref(), Some(index.as_path()));
        assert!(index.is_file());
    }

    #[test]
    fn every_bundled_language_has_a_highlighting_syntax() {
        let syntax_set = syntax_set();
        for language in [
            Language::Rust,
            Language::TypeScript,
            Language::Tsx,
            Language::JavaScript,
            Language::Bash,
            Language::Toml,
            Language::Yaml,
            Language::Json,
            Language::Markdown,
            Language::Sql,
        ] {
            assert!(
                syntax_for(language, syntax_set).is_some(),
                "missing syntax for {language}"
            );
        }
    }
}
