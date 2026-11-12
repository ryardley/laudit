use std::path::PathBuf;

use laudit_core::recon::graph::Graph;
use laudit_core::recon::parse_files;
use laudit_core::recon::walk::{walk_with, WalkOptions};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/graph-tree")
}

fn fixture_graph() -> Graph {
    let root = fixture_root();
    let files = walk_with(&root, &WalkOptions::default()).expect("walk fixture");
    let tags = parse_files(&root, &files).expect("parse fixture");
    Graph::build(&tags)
}

#[test]
fn fixture_ranks_hub_highest() {
    let graph = fixture_graph();
    let ranked = graph.ranked_files();
    assert_eq!(
        ranked[0].path,
        PathBuf::from("src/hub.rs"),
        "ranked files: {:?}",
        ranked
            .iter()
            .map(|file| (&file.path, file.rank))
            .collect::<Vec<_>>()
    );
    assert_eq!(graph.ranked_symbols()[0].name, "hub");
}

#[test]
fn fixture_communities_group_related_files() {
    let graph = fixture_graph();
    let community = |path: &str| {
        graph
            .file(&PathBuf::from(path))
            .expect("file node")
            .community
    };

    assert_eq!(community("src/hub.rs"), community("src/a1.rs"));
    assert_eq!(community("src/a1.rs"), community("src/a2.rs"));
    assert_eq!(community("src/a2.rs"), community("src/a3.rs"));
    assert_eq!(community("src/b1.rs"), community("src/b2.rs"));
    assert_eq!(community("src/b2.rs"), community("src/b3.rs"));
    assert_ne!(community("src/hub.rs"), community("src/b1.rs"));
}

#[test]
fn fixture_graph_is_deterministic() {
    assert_eq!(fixture_graph(), fixture_graph());
}
