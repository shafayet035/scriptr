//! Dependency graph: Kahn's algorithm into start waves, with cycle reporting.

use std::collections::{HashMap, HashSet};

/// A node and the ids it must start after. Edges to ids outside the node set
/// are ignored for ordering.
pub struct Node<'a> {
    pub id: &'a str,
    pub after: &'a [String],
}

/// Groups `nodes` into waves: every node's in-set dependencies live in an
/// earlier wave. Order within a wave follows input order (stable).
///
/// On a cycle returns `Err((from, to))`, an edge on the cycle where `from` is
/// the dependency and `to` the dependent (i.e. "`to` is after `from`").
pub fn waves(nodes: &[Node<'_>]) -> Result<Vec<Vec<String>>, (String, String)> {
    let ids: HashSet<&str> = nodes.iter().map(|n| n.id).collect();
    // Dependencies restricted to the node set, deduplicated.
    let deps: HashMap<&str, HashSet<&str>> = nodes
        .iter()
        .map(|n| {
            let d = n.after.iter().map(String::as_str).filter(|d| ids.contains(d) && *d != n.id);
            (n.id, d.collect())
        })
        .collect();
    // Self-dependency is the smallest possible cycle.
    if let Some(n) = nodes.iter().find(|n| n.after.iter().any(|d| d == n.id)) {
        return Err((n.id.to_string(), n.id.to_string()));
    }

    let mut indegree: HashMap<&str, usize> = deps.iter().map(|(id, d)| (*id, d.len())).collect();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for n in nodes {
        for d in &deps[n.id] {
            dependents.entry(*d).or_default().push(n.id);
        }
    }

    let mut out = Vec::new();
    let mut placed = 0;
    let mut current: Vec<&str> =
        nodes.iter().map(|n| n.id).filter(|id| indegree[id] == 0).collect();
    while !current.is_empty() {
        placed += current.len();
        let mut next = HashSet::new();
        for id in &current {
            for dep in dependents.get(id).into_iter().flatten() {
                let deg = indegree.get_mut(dep).expect("dependent is a node");
                *deg -= 1;
                if *deg == 0 {
                    next.insert(*dep);
                }
            }
        }
        out.push(current.iter().map(|s| s.to_string()).collect());
        current = nodes.iter().map(|n| n.id).filter(|id| next.contains(id)).collect();
    }

    if placed == nodes.len() {
        return Ok(out);
    }
    // Every unplaced node still has an unplaced dependency, so walking
    // dependencies from any of them must eventually revisit a node.
    let mut path: Vec<&str> = Vec::new();
    let mut cur = nodes.iter().map(|n| n.id).find(|id| indegree[id] > 0).expect("unplaced node");
    loop {
        if path.contains(&cur) {
            // `cur` was reached from the last node on the path, its dependent.
            let dependent = path.last().copied().unwrap_or(cur);
            return Err((cur.to_string(), dependent.to_string()));
        }
        path.push(cur);
        cur = deps[cur]
            .iter()
            .copied()
            .filter(|d| indegree[d] > 0)
            .min()
            .expect("unplaced node has an unplaced dependency");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(spec: &[(&str, &[&str])]) -> Result<Vec<Vec<String>>, (String, String)> {
        let afters: Vec<Vec<String>> =
            spec.iter().map(|(_, a)| a.iter().map(|s| s.to_string()).collect()).collect();
        let nodes: Vec<Node> =
            spec.iter().zip(&afters).map(|((id, _), after)| Node { id, after }).collect();
        waves(&nodes)
    }

    #[test]
    fn design_example_waves() {
        let w = run(&[
            ("db", &[]),
            ("stripe-mock", &[]),
            ("migrate", &["db"]),
            ("api", &["db", "migrate"]),
            ("worker", &["migrate"]),
            ("web", &["api"]),
        ])
        .unwrap();
        assert_eq!(
            w,
            vec![vec!["db", "stripe-mock"], vec!["migrate"], vec!["api", "worker"], vec!["web"]]
        );
    }

    #[test]
    fn external_edges_are_ignored() {
        let w = run(&[("api", &["db"]), ("web", &["api"])]).unwrap();
        assert_eq!(w, vec![vec!["api"], vec!["web"]]);
    }

    #[test]
    fn cycle_reports_an_edge_on_the_cycle() {
        let (from, to) = run(&[("db", &[]), ("a", &["db", "c"]), ("b", &["a"]), ("c", &["b"])])
            .unwrap_err();
        let cycle = [("a", "b"), ("b", "c"), ("c", "a")];
        assert!(cycle.contains(&(from.as_str(), to.as_str())), "{from} -> {to}");
    }

    #[test]
    fn two_node_cycle_and_self_loop() {
        let e = run(&[("a", &["b"]), ("b", &["a"])]).unwrap_err();
        assert!(e == ("a".into(), "b".into()) || e == ("b".into(), "a".into()));
        assert_eq!(run(&[("a", &["a"])]).unwrap_err(), ("a".into(), "a".into()));
    }
}
