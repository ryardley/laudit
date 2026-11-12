//! Deterministic file/symbol graph, PageRank centrality, and Louvain communities.
//!
//! The graph stage consumes the flat [`Tag`] list produced by [`super::parse`]
//! and turns it into the structural spine of a tour (D4): a directed file graph
//! (referencer -> definer, weighted by reference count), a symbol-level view,
//! PageRank centrality, and Louvain communities that become subsystems.
//!
//! Everything here is deterministic. Nodes and edges are sorted, iteration
//! order is fixed, and community labels are relabelled by first appearance so
//! repeated runs over the same tags produce identical results.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::parse::{Tag, TagKind};

/// PageRank damping factor.
const DAMPING: f64 = 0.85;
/// PageRank convergence threshold.
const PAGERANK_EPSILON: f64 = 1e-9;
/// Maximum PageRank iterations.
const PAGERANK_MAX_ITERATIONS: usize = 100;
/// Maximum Louvain passes per level.
const LOUVAIN_MAX_PASSES: usize = 100;
/// Numerical tolerance for Louvain moves.
const LOUVAIN_EPSILON: f64 = 1e-12;

/// A file in the structural graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileNode {
    /// Path relative to the repository root.
    pub path: PathBuf,
    /// PageRank centrality in the file graph.
    pub rank: f64,
    /// Louvain community (subsystem) the file belongs to.
    pub community: usize,
}

/// A symbol (definition) in the structural graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolNode {
    /// Stable symbol identifier.
    pub id: String,
    /// File that defines the symbol.
    pub file: PathBuf,
    /// Symbol name.
    pub name: String,
    /// 1-based line where the definition starts.
    pub line: u32,
    /// PageRank centrality in the symbol graph.
    pub rank: f64,
    /// Louvain community inherited from the defining file.
    pub community: usize,
}

/// A directed file edge: the `from` file references a symbol defined in `to`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEdge {
    /// Referencing file.
    pub from: PathBuf,
    /// Defining file.
    pub to: PathBuf,
    /// Number of references contributing to this edge.
    pub weight: u32,
}

/// A directed symbol edge: the `from` symbol references the `to` symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolEdge {
    /// Referencing symbol ID.
    pub from: String,
    /// Referenced symbol ID.
    pub to: String,
    /// Number of references contributing to this edge.
    pub weight: u32,
}

/// A Louvain community, exposed as a subsystem candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Community {
    /// Community identifier, contiguous and deterministic.
    pub id: usize,
    /// Files in the community, sorted by path.
    pub files: Vec<PathBuf>,
    /// Symbol IDs in the community, sorted by definition order.
    pub symbols: Vec<String>,
}

/// The deterministic structural graph built from parsed tags.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Graph {
    /// File nodes, sorted by path.
    pub files: Vec<FileNode>,
    /// Symbol nodes, sorted by file, line, then name.
    pub symbols: Vec<SymbolNode>,
    /// Directed file edges, sorted by `(from, to)`.
    pub file_edges: Vec<FileEdge>,
    /// Directed symbol edges, sorted by `(from, to)`.
    pub symbol_edges: Vec<SymbolEdge>,
    /// Communities, ordered by their deterministic identifier.
    pub communities: Vec<Community>,
}

impl Graph {
    /// Build the file graph, symbol graph, PageRank ranks, and communities.
    ///
    /// References are resolved to definitions by name. A reference prefers a
    /// definition in the same file; otherwise it resolves to every definition
    /// with that name. Unresolved references (external symbols) are ignored.
    #[must_use]
    pub fn build(tags: &[Tag]) -> Self {
        let mut tags = tags.to_vec();
        tags.sort();
        tags.dedup();

        let mut def_keys: BTreeSet<(PathBuf, u32, String)> = BTreeSet::new();
        let mut file_paths: BTreeSet<PathBuf> = BTreeSet::new();
        for tag in &tags {
            file_paths.insert(tag.file.clone());
            if tag.kind == TagKind::Def {
                def_keys.insert((tag.file.clone(), tag.line, tag.name.clone()));
            }
        }

        let mut symbols: Vec<SymbolNode> = def_keys
            .iter()
            .map(|(file, line, name)| SymbolNode {
                id: symbol_id(file, *line, name),
                file: file.clone(),
                name: name.clone(),
                line: *line,
                rank: 0.0,
                community: 0,
            })
            .collect();

        let mut files: Vec<FileNode> = file_paths
            .iter()
            .map(|path| FileNode {
                path: path.clone(),
                rank: 0.0,
                community: 0,
            })
            .collect();

        let file_index: BTreeMap<PathBuf, usize> = files
            .iter()
            .enumerate()
            .map(|(index, file)| (file.path.clone(), index))
            .collect();

        let mut by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (index, symbol) in symbols.iter().enumerate() {
            by_name.entry(symbol.name.clone()).or_default().push(index);
        }

        let mut defs_by_file: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
        for (index, symbol) in symbols.iter().enumerate() {
            defs_by_file
                .entry(symbol.file.clone())
                .or_default()
                .push(index);
        }

        let mut file_edge_weights: BTreeMap<(usize, usize), u32> = BTreeMap::new();
        let mut symbol_edge_weights: BTreeMap<(usize, usize), u32> = BTreeMap::new();

        for tag in &tags {
            if tag.kind != TagKind::Ref {
                continue;
            }
            let Some(targets) = by_name.get(&tag.name) else {
                continue;
            };
            let Some(&from_file) = file_index.get(&tag.file) else {
                continue;
            };

            let same_file: Vec<usize> = targets
                .iter()
                .copied()
                .filter(|&target| symbols[target].file == tag.file)
                .collect();
            let resolved: &[usize] = if same_file.is_empty() {
                targets.as_slice()
            } else {
                same_file.as_slice()
            };

            let from_symbol = enclosing_symbol(&defs_by_file, &symbols, &tag.file, tag.line);

            let mut target_files: BTreeSet<usize> = BTreeSet::new();
            for &to_symbol in resolved {
                target_files.insert(file_index[&symbols[to_symbol].file]);
                if let Some(from_symbol) = from_symbol {
                    if from_symbol != to_symbol {
                        *symbol_edge_weights
                            .entry((from_symbol, to_symbol))
                            .or_insert(0) += 1;
                    }
                }
            }
            for to_file in target_files {
                if from_file != to_file {
                    *file_edge_weights.entry((from_file, to_file)).or_insert(0) += 1;
                }
            }
        }

        let file_edges: Vec<FileEdge> = file_edge_weights
            .into_iter()
            .map(|((from, to), weight)| FileEdge {
                from: files[from].path.clone(),
                to: files[to].path.clone(),
                weight,
            })
            .collect();

        let symbol_edges: Vec<SymbolEdge> = symbol_edge_weights
            .into_iter()
            .map(|((from, to), weight)| SymbolEdge {
                from: symbols[from].id.clone(),
                to: symbols[to].id.clone(),
                weight,
            })
            .collect();

        let file_rank_edges: Vec<(usize, usize, f64)> = file_edges
            .iter()
            .map(|edge| {
                (
                    file_index[&edge.from],
                    file_index[&edge.to],
                    f64::from(edge.weight),
                )
            })
            .collect();
        let file_ranks = pagerank(files.len(), &file_rank_edges);
        for (file, rank) in files.iter_mut().zip(file_ranks) {
            file.rank = rank;
        }

        let symbol_index: BTreeMap<String, usize> = symbols
            .iter()
            .enumerate()
            .map(|(index, symbol)| (symbol.id.clone(), index))
            .collect();
        let symbol_rank_edges: Vec<(usize, usize, f64)> = symbol_edges
            .iter()
            .map(|edge| {
                (
                    symbol_index[&edge.from],
                    symbol_index[&edge.to],
                    f64::from(edge.weight),
                )
            })
            .collect();
        let symbol_ranks = pagerank(symbols.len(), &symbol_rank_edges);
        for (symbol, rank) in symbols.iter_mut().zip(symbol_ranks) {
            symbol.rank = rank;
        }

        let undirected_edges: Vec<(usize, usize, f64)> = file_edges
            .iter()
            .map(|edge| {
                (
                    file_index[&edge.from],
                    file_index[&edge.to],
                    f64::from(edge.weight),
                )
            })
            .collect();
        let file_communities = louvain(files.len(), &undirected_edges);
        for (file, community) in files.iter_mut().zip(file_communities) {
            file.community = community;
        }
        for symbol in &mut symbols {
            if let Some(&file) = file_index.get(&symbol.file) {
                symbol.community = files[file].community;
            }
        }

        let community_count = files
            .iter()
            .map(|file| file.community)
            .max()
            .map_or(0, |community| community + 1);
        let mut communities: Vec<Community> = (0..community_count)
            .map(|id| Community {
                id,
                files: Vec::new(),
                symbols: Vec::new(),
            })
            .collect();
        for file in &files {
            communities[file.community].files.push(file.path.clone());
        }
        for symbol in &symbols {
            communities[symbol.community]
                .symbols
                .push(symbol.id.clone());
        }

        Self {
            files,
            symbols,
            file_edges,
            symbol_edges,
            communities,
        }
    }

    /// Files ordered by PageRank descending, then path ascending.
    #[must_use]
    pub fn ranked_files(&self) -> Vec<&FileNode> {
        let mut ranked: Vec<&FileNode> = self.files.iter().collect();
        ranked.sort_by(|a, b| {
            b.rank
                .partial_cmp(&a.rank)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.path.cmp(&b.path))
        });
        ranked
    }

    /// Symbols ordered by PageRank descending, then definition order.
    #[must_use]
    pub fn ranked_symbols(&self) -> Vec<&SymbolNode> {
        let mut ranked: Vec<&SymbolNode> = self.symbols.iter().collect();
        ranked.sort_by(|a, b| {
            b.rank
                .partial_cmp(&a.rank)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.file.cmp(&b.file))
                .then_with(|| a.line.cmp(&b.line))
                .then_with(|| a.name.cmp(&b.name))
        });
        ranked
    }

    /// Look up a file node by repository-relative path.
    #[must_use]
    pub fn file(&self, path: &Path) -> Option<&FileNode> {
        self.files.iter().find(|file| file.path == path)
    }

    /// Look up a symbol node by its stable identifier.
    #[must_use]
    pub fn symbol(&self, id: &str) -> Option<&SymbolNode> {
        self.symbols.iter().find(|symbol| symbol.id == id)
    }

    /// Look up a community by its identifier.
    #[must_use]
    pub fn community(&self, id: usize) -> Option<&Community> {
        self.communities.get(id)
    }
}

fn enclosing_symbol(
    defs_by_file: &BTreeMap<PathBuf, Vec<usize>>,
    symbols: &[SymbolNode],
    file: &Path,
    line: u32,
) -> Option<usize> {
    let defs = defs_by_file.get(file)?;
    let mut best = None;
    for &index in defs {
        if symbols[index].line <= line {
            best = Some(index);
        } else {
            break;
        }
    }
    best
}

fn symbol_id(file: &Path, line: u32, name: &str) -> String {
    let key = format!("{}|{}|{}", file.display(), line, name);
    format!("sym-{:016x}", fnv1a(key.as_bytes()))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn pagerank(n: usize, edges: &[(usize, usize, f64)]) -> Vec<f64> {
    if n == 0 {
        return Vec::new();
    }

    let mut out_weight = vec![0.0_f64; n];
    let mut incoming: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    for &(from, to, weight) in edges {
        if from == to || weight <= 0.0 {
            continue;
        }
        out_weight[from] += weight;
        incoming[to].push((from, weight));
    }

    let n_f = n as f64;
    let mut rank = vec![1.0 / n_f; n];
    let mut next = vec![0.0_f64; n];

    for _ in 0..PAGERANK_MAX_ITERATIONS {
        let dangling: f64 = rank
            .iter()
            .zip(out_weight.iter())
            .filter(|(_, weight)| **weight == 0.0)
            .map(|(rank, _)| *rank)
            .sum();
        let base = (1.0 - DAMPING) / n_f + DAMPING * dangling / n_f;
        next.fill(base);
        for (to, sources) in incoming.iter().enumerate() {
            for &(from, weight) in sources {
                next[to] += DAMPING * rank[from] * weight / out_weight[from];
            }
        }
        let diff: f64 = next
            .iter()
            .zip(rank.iter())
            .map(|(next, rank)| (*next - *rank).abs())
            .sum();
        std::mem::swap(&mut rank, &mut next);
        if diff < PAGERANK_EPSILON {
            break;
        }
    }

    rank
}

struct WeightedGraph {
    n: usize,
    adj: Vec<BTreeMap<usize, f64>>,
    degree: Vec<f64>,
}

impl WeightedGraph {
    fn from_edges(n: usize, edges: &[(usize, usize, f64)]) -> Self {
        let mut adj: Vec<BTreeMap<usize, f64>> = vec![BTreeMap::new(); n];
        for &(u, v, weight) in edges {
            if u >= n || v >= n || weight <= 0.0 {
                continue;
            }
            if u == v {
                *adj[u].entry(u).or_insert(0.0) += weight;
            } else {
                *adj[u].entry(v).or_insert(0.0) += weight;
                *adj[v].entry(u).or_insert(0.0) += weight;
            }
        }
        let degree = adj
            .iter()
            .enumerate()
            .map(|(node, neighbors)| {
                neighbors
                    .iter()
                    .map(|(&neighbor, &weight)| {
                        if neighbor == node {
                            2.0 * weight
                        } else {
                            weight
                        }
                    })
                    .sum()
            })
            .collect();
        Self { n, adj, degree }
    }

    fn one_level(&self) -> (Vec<usize>, bool) {
        let n = self.n;
        let m2: f64 = self.degree.iter().sum();
        if n == 0 || m2 <= 0.0 {
            return ((0..n).collect(), false);
        }

        let mut membership: Vec<usize> = (0..n).collect();
        let mut total: Vec<f64> = self.degree.clone();
        let mut moved_any = false;

        for _ in 0..LOUVAIN_MAX_PASSES {
            let mut moved = false;
            for (node, neighbors) in self.adj.iter().enumerate() {
                let degree = self.degree[node];
                let current = membership[node];

                let mut weights_to_community: BTreeMap<usize, f64> = BTreeMap::new();
                for (&neighbor, &weight) in neighbors {
                    if neighbor == node {
                        continue;
                    }
                    let community = membership[neighbor];
                    *weights_to_community.entry(community).or_insert(0.0) += weight;
                }

                total[current] -= degree;
                let mut best = current;
                let mut best_score = weights_to_community.get(&current).copied().unwrap_or(0.0)
                    - degree * total[current] / m2;
                for (&community, &weight) in &weights_to_community {
                    if community == current {
                        continue;
                    }
                    let score = weight - degree * total[community] / m2;
                    if score > best_score + LOUVAIN_EPSILON {
                        best_score = score;
                        best = community;
                    }
                }

                membership[node] = best;
                total[best] += degree;
                if best != current {
                    moved = true;
                    moved_any = true;
                }
            }
            if !moved {
                break;
            }
        }

        (relabel(&membership), moved_any)
    }

    fn aggregate(&self, communities: &[usize], new_n: usize) -> Self {
        let mut adj: Vec<BTreeMap<usize, f64>> = vec![BTreeMap::new(); new_n];
        for (node, neighbors) in self.adj.iter().enumerate() {
            let from_community = communities[node];
            for (&neighbor, &weight) in neighbors {
                if neighbor < node {
                    continue;
                }
                let to_community = communities[neighbor];
                if from_community == to_community {
                    *adj[from_community].entry(from_community).or_insert(0.0) += weight;
                } else {
                    *adj[from_community].entry(to_community).or_insert(0.0) += weight;
                    *adj[to_community].entry(from_community).or_insert(0.0) += weight;
                }
            }
        }
        let degree = adj
            .iter()
            .enumerate()
            .map(|(node, neighbors)| {
                neighbors
                    .iter()
                    .map(|(&neighbor, &weight)| {
                        if neighbor == node {
                            2.0 * weight
                        } else {
                            weight
                        }
                    })
                    .sum()
            })
            .collect();
        Self {
            n: new_n,
            adj,
            degree,
        }
    }
}

fn louvain(n: usize, edges: &[(usize, usize, f64)]) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }

    let mut graph = WeightedGraph::from_edges(n, edges);
    let mut membership: Vec<usize> = (0..n).collect();

    loop {
        let (communities, moved) = graph.one_level();
        for community in &mut membership {
            *community = communities[*community];
        }
        if !moved {
            break;
        }
        let new_n = communities
            .iter()
            .copied()
            .max()
            .map_or(0, |community| community + 1);
        if new_n == graph.n {
            break;
        }
        graph = graph.aggregate(&communities, new_n);
    }

    relabel(&membership)
}

fn relabel(membership: &[usize]) -> Vec<usize> {
    let mut mapping: BTreeMap<usize, usize> = BTreeMap::new();
    let mut next = 0;
    let mut relabelled = Vec::with_capacity(membership.len());
    for &community in membership {
        let id = *mapping.entry(community).or_insert_with(|| {
            let id = next;
            next += 1;
            id
        });
        relabelled.push(id);
    }
    relabelled
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(file: &str, line: u32, name: &str, kind: TagKind) -> Tag {
        Tag {
            file: PathBuf::from(file),
            line,
            name: name.to_owned(),
            kind,
        }
    }

    fn hub_tags() -> Vec<Tag> {
        vec![
            tag("src/hub.rs", 1, "hub", TagKind::Def),
            tag("src/a1.rs", 1, "a1", TagKind::Def),
            tag("src/a1.rs", 2, "hub", TagKind::Ref),
            tag("src/a1.rs", 2, "a2", TagKind::Ref),
            tag("src/a1.rs", 2, "a3", TagKind::Ref),
            tag("src/a2.rs", 1, "a2", TagKind::Def),
            tag("src/a2.rs", 2, "hub", TagKind::Ref),
            tag("src/a2.rs", 2, "a1", TagKind::Ref),
            tag("src/a2.rs", 2, "a3", TagKind::Ref),
            tag("src/a3.rs", 1, "a3", TagKind::Def),
            tag("src/a3.rs", 2, "hub", TagKind::Ref),
            tag("src/a3.rs", 2, "a1", TagKind::Ref),
            tag("src/a3.rs", 2, "a2", TagKind::Ref),
            tag("src/b1.rs", 1, "b1", TagKind::Def),
            tag("src/b1.rs", 2, "b2", TagKind::Ref),
            tag("src/b2.rs", 1, "b2", TagKind::Def),
            tag("src/b2.rs", 2, "b3", TagKind::Ref),
            tag("src/b3.rs", 1, "b3", TagKind::Def),
            tag("src/main.rs", 1, "main", TagKind::Def),
            tag("src/main.rs", 2, "hub", TagKind::Ref),
            tag("src/main.rs", 2, "a1", TagKind::Ref),
            tag("src/main.rs", 2, "b1", TagKind::Ref),
        ]
    }

    #[test]
    fn fixture_ranks_hub_highest() {
        let graph = Graph::build(&hub_tags());
        let ranked = graph.ranked_files();
        assert_eq!(ranked[0].path, PathBuf::from("src/hub.rs"));
        assert_eq!(graph.ranked_symbols()[0].name, "hub");
    }

    #[test]
    fn communities_group_related_files() {
        let graph = Graph::build(&hub_tags());
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
    fn results_are_deterministic() {
        let first = Graph::build(&hub_tags());
        let second = Graph::build(&hub_tags());
        assert_eq!(first, second);
    }

    #[test]
    fn file_edges_are_weighted_by_reference_count() {
        let tags = vec![
            tag("src/lib.rs", 1, "helper", TagKind::Def),
            tag("src/main.rs", 1, "main", TagKind::Def),
            tag("src/main.rs", 2, "helper", TagKind::Ref),
            tag("src/main.rs", 3, "helper", TagKind::Ref),
        ];
        let graph = Graph::build(&tags);
        let edge = graph
            .file_edges
            .iter()
            .find(|edge| edge.from == Path::new("src/main.rs"))
            .expect("file edge");
        assert_eq!(edge.to, PathBuf::from("src/lib.rs"));
        assert_eq!(edge.weight, 2);
    }

    #[test]
    fn symbol_edges_link_referencing_symbol_to_definition() {
        let graph = Graph::build(&hub_tags());
        let a1 = graph
            .symbols
            .iter()
            .find(|symbol| symbol.name == "a1")
            .expect("a1");
        let hub = graph
            .symbols
            .iter()
            .find(|symbol| symbol.name == "hub")
            .expect("hub");
        assert!(graph
            .symbol_edges
            .iter()
            .any(|edge| edge.from == a1.id && edge.to == hub.id));
    }

    #[test]
    fn unresolved_references_are_ignored() {
        let tags = vec![
            tag("src/main.rs", 1, "main", TagKind::Def),
            tag("src/main.rs", 2, "external", TagKind::Ref),
        ];
        let graph = Graph::build(&tags);
        assert!(graph.file_edges.is_empty());
        assert!(graph.symbol_edges.is_empty());
    }

    #[test]
    fn empty_tags_produce_empty_graph() {
        let graph = Graph::build(&[]);
        assert!(graph.files.is_empty());
        assert!(graph.symbols.is_empty());
        assert!(graph.file_edges.is_empty());
        assert!(graph.symbol_edges.is_empty());
        assert!(graph.communities.is_empty());
    }

    #[test]
    fn same_file_reference_prefers_local_definition() {
        let tags = vec![
            tag("src/a.rs", 1, "helper", TagKind::Def),
            tag("src/b.rs", 1, "helper", TagKind::Def),
            tag("src/a.rs", 2, "helper", TagKind::Ref),
        ];
        let graph = Graph::build(&tags);
        assert!(graph.file_edges.is_empty());
    }
}
