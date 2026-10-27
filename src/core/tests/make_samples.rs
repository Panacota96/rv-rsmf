//! Regenerates the synthetic Teams (.msg) and CSV samples under examples/sample/evidence:
//! `cargo test -p rv-rsmf-core --test make_samples -- --ignored`. Output is deterministic.

#[path = "support/make_msg.rs"]
mod make_msg;
#[path = "support/samples.rs"]
mod samples;

use std::path::Path;

#[test]
#[ignore = "writes into examples/; run explicitly to regenerate the samples"]
fn make_samples() {
    let ev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/sample/evidence");
    for (rel, bytes) in samples::files() {
        let p = ev.join(&rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        println!("wrote {}", p.display());
    }
}
