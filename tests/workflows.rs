//! Guards on the repo's GitHub workflow files.

use std::path::Path;

fn workflows() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/workflows");
    let mut out: Vec<(String, String)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "yml" || e == "yaml"))
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().to_string(),
                std::fs::read_to_string(&p).unwrap(),
            )
        })
        .collect();
    out.sort();
    assert!(!out.is_empty());
    out
}

#[test]
fn every_workflow_declares_token_permissions() {
    // Without a `permissions:` block the GITHUB_TOKEN gets the repo default,
    // which may be read/write on everything.
    for (name, text) in workflows() {
        assert!(
            text.lines()
                .any(|l| l.trim_start().starts_with("permissions:")),
            "{name} has no permissions: block"
        );
    }
}

#[test]
fn reusable_workflows_are_pinned_to_a_commit() {
    // A reusable workflow called with `secrets: inherit` gets every secret;
    // a mutable tag ref would let whoever moves the tag run code with them.
    for (name, text) in workflows() {
        for line in text.lines() {
            let Some(uses) = line.trim_start().strip_prefix("uses:") else {
                continue;
            };
            let uses = uses.split('#').next().unwrap().trim();
            if !uses.contains("/.github/workflows/") || uses.starts_with("./") {
                continue;
            }
            let rev = uses.rsplit('@').next().unwrap();
            assert!(
                rev.len() == 40 && rev.chars().all(|c| c.is_ascii_hexdigit()),
                "{name}: `{uses}` must be pinned to a full commit SHA"
            );
        }
    }
}
