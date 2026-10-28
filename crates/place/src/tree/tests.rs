use std::sync::Arc;

use super::*;

fn source(text: &str) -> Source {
    Source::new(Arc::from(text))
}

/// A fresh tree with a module at each path, the way a session mounts its outputs.
fn mounted(scripts: &[(&str, &str)]) -> Tree {
    let mut tree = Tree::new();
    for &(path, text) in scripts {
        tree.set_script(path, ScriptKind::Module, source(text), None).expect("a valid path");
    }
    tree
}

fn live(scripts: &[(&str, &str)]) -> Tree {
    let mut tree = Tree::new();
    tree.update(mounted(scripts));
    tree
}

#[test]
fn renamed_folder_keeps_its_subtree_ids() {
    let mut tree = live(&[("ReplicatedStorage/shared/a/m", "return 1"), ("ReplicatedStorage/shared/a/n", "return 2")]);
    let (a, m) = (tree.find("ReplicatedStorage/shared/a").unwrap(), tree.find("ReplicatedStorage/shared/a/m").unwrap());
    let structural = tree
        .update(mounted(&[("ReplicatedStorage/shared/b/m", "return 1"), ("ReplicatedStorage/shared/b/n", "return 2")]));
    assert!(structural);
    // the renamed folder has the same hash (its own name is not hashed), so it claims the old ids
    assert_eq!(tree.find("ReplicatedStorage/shared/b"), Some(a));
    assert_eq!(tree.find("ReplicatedStorage/shared/b/m"), Some(m));
    assert!(tree.find("ReplicatedStorage/shared/a").is_none());
}

#[test]
fn class_change_is_a_new_id_whose_children_pair_with_the_old_ones() {
    let mut tree = live(&[("ReplicatedStorage/shared/x/child", "return 1")]);
    let (x, child) =
        (tree.find("ReplicatedStorage/shared/x").unwrap(), tree.find("ReplicatedStorage/shared/x/child").unwrap());
    assert_eq!(tree.get(x).unwrap().class, "Folder");
    // x becomes the module (init file): a ModuleScript where a Folder was
    tree.update(mounted(&[
        ("ReplicatedStorage/shared/x", "return 0"),
        ("ReplicatedStorage/shared/x/child", "return 1"),
    ]));
    let now = tree.find("ReplicatedStorage/shared/x").unwrap();
    assert_ne!(now, x);
    assert_eq!(tree.get(now).unwrap().class, "ModuleScript");
    assert_eq!(tree.find("ReplicatedStorage/shared/x/child"), Some(child));
    assert!(tree.get(x).is_none());
}

#[test]
fn set_sources_hashes_like_a_full_update() {
    let before = [("ReplicatedStorage/shared/a/m", "return 1"), ("ReplicatedStorage/shared/n", "return 2")];
    let after = [("ReplicatedStorage/shared/a/m", "return 3"), ("ReplicatedStorage/shared/n", "return 2")];
    let mut edited = live(&before);
    let m = edited.find("ReplicatedStorage/shared/a/m").unwrap();
    let old = edited.clone();
    assert_eq!(edited.set_sources(vec![(m, source("return 3"))]), vec![m]);
    let full = live(&after);
    for path in [
        "",
        "ReplicatedStorage",
        "ReplicatedStorage/shared",
        "ReplicatedStorage/shared/a",
        "ReplicatedStorage/shared/n",
    ] {
        let (a, b) = (edited.find(path).unwrap(), full.find(path).unwrap());
        assert_eq!(edited.get(a).unwrap().hash, full.get(b).unwrap().hash, "{path}");
    }
    // only the edited node and its ancestors changed, so a subscriber's diff walks just those
    let mut changed = edited.differs(&old);
    changed.sort();
    let mut expected: Vec<Id> = ["", "ReplicatedStorage", "ReplicatedStorage/shared", "ReplicatedStorage/shared/a"]
        .iter()
        .map(|p| edited.find(p).unwrap())
        .chain([m])
        .collect();
    expected.sort();
    assert_eq!(changed, expected);
}

/// `ReplicatedStorage/shared` holding StringValues all named `dup`, with these values, in order.
fn dups(values: &[&str]) -> Tree {
    let mut fresh = Tree::new();
    let shared = fresh.folder_path("x", &["ReplicatedStorage", "shared"]).unwrap();
    for &value in values {
        let mut node = Node::new(shared, "dup", "StringValue", Origin::Place);
        node.set_prop(ustr("Value"), Variant::String(value.into()));
        fresh.add(shared, node);
    }
    fresh
}

#[test]
fn a_duplicate_group_that_changes_size_is_new_whole() {
    let mut tree = Tree::new();
    tree.update(dups(&["b", "c"]));
    let old: Vec<Id> = tree.node(tree.find("ReplicatedStorage/shared").unwrap()).children.clone();
    // a member added in front: Studio appends what it inserts, so an old id kept by hash (b, c) would sit at another
    // ordinal than the tree gives it, and a later hydrate would map the ids crosswise
    tree.update(dups(&["a", "b", "c"]));
    let now = &tree.node(tree.find("ReplicatedStorage/shared").unwrap()).children;
    assert_eq!(now.len(), 3);
    assert!(now.iter().all(|a| !old.contains(a)), "{old:?} -> {now:?}");
}
