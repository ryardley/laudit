use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use laudit_core::comprehension::comprehend;
use laudit_core::config::Config;
use laudit_core::llm::LlmClient;
use laudit_core::model::{CodebaseModel, Skeleton, Stop, Tour};
use laudit_core::plan::{plan, plan_from_artifacts, PlanError, PlanOptions, TOURS_DIR};
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("laudit-plan-it-{}-{id}", std::process::id()));
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

fn fixture() -> TempDir {
    let temp = TempDir::new();
    temp.write(
        "src/main.rs",
        "fn main() {\n    helper();\n}\n\nfn helper() {\n    println!(\"hi\");\n}\n",
    );
    temp.write("tests/it.rs", "fn it_works() {\n    assert!(true);\n}\n");
    temp.write("config/app.toml", "[server]\nport = 8080\n");
    temp.write("scripts/deploy.sh", "deploy() {\n    echo deploying\n}\n");
    temp
}

fn config_for(temp: &TempDir, server: &MockServer) -> Config {
    let path = temp.path().join(".laudit/config.toml");
    fs::create_dir_all(path.parent().expect("config parent")).expect("create .laudit");
    fs::write(
        &path,
        format!(
            "[provider]\nbase_url = \"{}\"\nmodel = \"test-model\"\n",
            server.uri()
        ),
    )
    .expect("write config");
    Config::load_from(None, Some(&path)).expect("load config")
}

fn completion(content: &str) -> Value {
    json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": content },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 5,
            "total_tokens": 15
        }
    })
}

fn symbol_id(skeleton: &Skeleton, name: &str) -> String {
    skeleton
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("missing symbol {name}"))
        .id
        .clone()
}

fn model_response(skeleton: &Skeleton) -> Value {
    let main = symbol_id(skeleton, "main");
    let helper = symbol_id(skeleton, "helper");
    let test = symbol_id(skeleton, "it_works");
    json!({
        "subsystems": [{
            "id": "core",
            "title": "Core",
            "summary": "Core subsystem.",
            "symbol_ids": [main, helper, test],
            "files": ["src/main.rs", "tests/it.rs"]
        }],
        "chapters": [],
        "flows": [],
        "glossary": []
    })
}

fn all_stops(tour: &Tour) -> Vec<&Stop> {
    tour.chapters
        .iter()
        .flat_map(|chapter| chapter.stops.iter())
        .collect()
}

fn load_model(temp: &TempDir) -> CodebaseModel {
    serde_json::from_slice(&fs::read(temp.path().join(".laudit/model.json")).expect("read model"))
        .expect("parse model")
}

#[tokio::test]
async fn mock_llm_run_emits_tour_with_valid_ranges_and_skips_tests() {
    let temp = fixture();
    let server = MockServer::start().await;
    let config = config_for(&temp, &server);
    let skeleton = laudit_core::build::recon(temp.path(), &config)
        .expect("recon")
        .skeleton;
    laudit_core::build::write_skeleton(temp.path(), &skeleton).expect("write skeleton");

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(completion(&model_response(&skeleton).to_string())),
        )
        .mount(&server)
        .await;

    let client = LlmClient::new(server.uri(), "test-model", None).expect("client");
    comprehend(temp.path(), &config, &skeleton, &client)
        .await
        .expect("comprehend");

    let report = plan_from_artifacts(temp.path(), &config, &PlanOptions::default())
        .expect("plan from artifacts");

    assert!(report.tour_path.is_file());
    assert!(report.tour_path.starts_with(temp.path().join(TOURS_DIR)));

    let persisted: Tour =
        serde_json::from_slice(&fs::read(&report.tour_path).expect("read tour")).expect("parse");
    persisted.validate_schema_version().expect("schema version");
    assert_eq!(persisted, report.tour);

    let stops = all_stops(&persisted);
    assert!(!stops.is_empty());
    for stop in &stops {
        let contents = fs::read_to_string(temp.path().join(&stop.file)).expect("read stop file");
        let line_count = contents.lines().count() as u32;
        assert!(stop.start_line >= 1, "bad start line: {stop:?}");
        assert!(stop.end_line >= stop.start_line, "bad range: {stop:?}");
        assert!(stop.end_line <= line_count, "range past EOF: {stop:?}");
        assert!(!stop.symbol_id.is_empty());
    }

    assert!(
        !stops.iter().any(|stop| stop.file == "tests/it.rs"),
        "tests should be excluded by default"
    );
    assert!(stops.iter().any(|stop| stop.file == "config/app.toml"));
    assert!(stops.iter().any(|stop| stop.file == "scripts/deploy.sh"));

    let model = load_model(&temp);
    let with_tests = plan(
        temp.path(),
        &config,
        &skeleton,
        &model,
        &PlanOptions {
            include_tests: true,
            include_config: true,
        },
    )
    .expect("plan with tests");
    assert!(all_stops(&with_tests.tour)
        .iter()
        .any(|stop| stop.file == "tests/it.rs"));
}

#[test]
fn invalid_ranges_are_rejected() {
    let temp = fixture();
    let config = Config::load_from_offline(None, None).expect("offline config");
    let mut skeleton = laudit_core::build::recon(temp.path(), &config)
        .expect("recon")
        .skeleton;
    let main = skeleton
        .symbols
        .iter_mut()
        .find(|symbol| symbol.name == "main")
        .expect("main symbol");
    main.line = 999;

    let model = CodebaseModel {
        schema_version: laudit_core::model::SCHEMA_VERSION,
        tool_version: "0.1.0".to_owned(),
        repo_id: skeleton.repo_id.clone(),
        head: skeleton.head.clone(),
        dirty_hash: skeleton.dirty_hash.clone(),
        generator: "laudit".to_owned(),
        model: "test-model".to_owned(),
        prompt_version: "1".to_owned(),
        generated_at: "2026-11-12T00:00:00Z".to_owned(),
        subsystems: vec![laudit_core::model::Subsystem {
            id: "core".to_owned(),
            title: "Core".to_owned(),
            summary: "Core.".to_owned(),
            symbol_ids: vec![main.id.clone()],
            files: vec!["src/main.rs".to_owned()],
        }],
        chapters: Vec::new(),
        flows: Vec::new(),
        glossary: Vec::new(),
    };

    let error = plan(
        temp.path(),
        &config,
        &skeleton,
        &model,
        &PlanOptions::default(),
    )
    .expect_err("invalid range");
    assert!(matches!(error, PlanError::InvalidRange { .. }));
}
