//! End-to-end integration test: the full five-phase pipeline on the fixture
//! jars (spec §12 definition of done, offline-achievable subset).

use auto_porter_lib::{analyze, port, verify_dir, PortOptions};
use shared::config::PorterConfig;
use shared::report::{CheckStatus, Overall};
use std::path::{Path, PathBuf};

fn ws(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join(rel)
}

#[test]
fn analyze_writes_deterministic_artifacts() {
    let jar = ws("tests/fixtures/jars/veinminer-fabric-1.0.0.jar");
    let out = std::env::temp_dir().join(format!("ap-analyze-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);

    let classified = analyze(&jar, &out, None).expect("analyze succeeds");
    assert!(out.join("decomposed.json").is_file());
    assert!(out.join("classified.json").is_file());

    // the Fabric block-break callback resolves to the destroy_block hook
    assert!(classified
        .plan
        .features
        .iter()
        .any(|f| f.hook.as_ref().is_some_and(|h| h.name == "destroy_block")));
    // every unresolved hook would be a mapping bug
    assert!(classified.unresolved_hooks.is_empty());

    // deterministic: two runs produce byte-identical artifacts
    let out2 = std::env::temp_dir().join(format!("ap-analyze2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out2);
    analyze(&jar, &out2, None).expect("second analyze");
    for name in ["decomposed.json", "classified.json"] {
        assert_eq!(
            std::fs::read(out.join(name)).unwrap(),
            std::fs::read(out2.join(name)).unwrap(),
            "{name} must be deterministic"
        );
        // and match the committed snapshot
        assert_eq!(
            std::fs::read(out.join(name)).unwrap(),
            std::fs::read(ws(&format!(
                "tests/fixtures/expected/veinminer-fabric-1.0.0/{name}"
            )))
            .unwrap(),
            "{name} diverged from the committed snapshot"
        );
    }
    let _ = std::fs::remove_dir_all(&out);
    let _ = std::fs::remove_dir_all(&out2);
}

#[test]
fn full_pipeline_on_veinminer_fixture() {
    let jar = ws("tests/fixtures/jars/veinminer-fabric-1.0.0.jar");
    let tmp = std::env::temp_dir().join(format!("ap-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);

    let cfg = PorterConfig::default();
    let out_dir = tmp.join("veinminer");
    let opts = PortOptions {
        horizon_revision: None,
        data_only: false,
        output: out_dir.clone(),
        verbose: false,
        hooks_json: None,
    };

    let ported = port(&jar, &cfg, &opts).expect("port succeeds");
    assert_eq!(ported.report.mod_id, "veinminer");

    // §7 output contract
    assert!(out_dir.join("veinminer.hmod").is_file(), ".hmod emitted");
    assert!(out_dir.join("veinminer.hnmod").is_file(), ".hnmod emitted");
    assert!(
        out_dir.join("veinminer-report.json").is_file(),
        "report emitted"
    );
    assert!(out_dir.join("veinminer-tests").is_dir(), "tests emitted");

    // whichever carrier is available must be recorded honestly: the real
    // horizon-port-pack when installed (CI, dev machines with the pinned
    // checkout), the built-in fallback otherwise
    let carrier = ported.report.data_carry.as_deref().unwrap_or_default();
    assert!(
        carrier == "built-in-fallback" || carrier == "horizon-port-pack",
        "honest carrier, got {carrier}"
    );
    // the bundle generation ran (2 ported: data + the block-break feature)
    assert_eq!(ported.report.summary.ported, 2);
    // offline the revision is the placeholder, so PASS_WITH_WARNINGS is the
    // best achievable overall (spec §12: report lists what works)
    assert_eq!(ported.report.overall, Overall::PassWithWarnings);

    // ---- verify --static
    let verification =
        verify_dir(&out_dir, &cfg, true, false, false, false, false).expect("verify succeeds");
    assert_ne!(verification.r#static.schema, CheckStatus::Fail);
    assert_ne!(verification.r#static.hooks, CheckStatus::Fail);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn data_only_mode_skips_code_generation() {
    let jar = ws("tests/fixtures/jars/moreassets-fabric-2.1.0.jar");
    let tmp = std::env::temp_dir().join(format!("ap-dataonly-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);

    let cfg = PorterConfig::default();
    let out_dir = tmp.join("moreassets");
    let opts = PortOptions {
        horizon_revision: None,
        data_only: true,
        output: out_dir.clone(),
        verbose: false,
        hooks_json: None,
    };
    let ported = port(&jar, &cfg, &opts).expect("data-only port");
    assert!(out_dir.join("moreassets.hmod").is_file());
    assert!(
        !out_dir.join("moreassets.hnmod").exists(),
        "no .hnmod for data-only"
    );
    assert_eq!(ported.report.overall, Overall::Pass);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn verify_rejects_foreign_top_level_entries_in_hnmod() {
    // a .hnmod carrying a top-level entry beyond horizon-native.json + mixins/
    // must be refused (docs/api/mod-files.md)
    use std::io::Write as _;
    let tmp = std::env::temp_dir().join(format!("ap-hnmod-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let path = tmp.join("bad.hnmod");
    let file = std::fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
    zip.start_file("horizon-native.json", opts).unwrap();
    zip.write_all(
        br#"{"schema":1,"id":"bad","version":"0.1.0","game":"26.3","horizon":"1111111111111111111111111111111111111111","mixins":"2222222222222222222222222222222222222222"}"#,
    )
    .unwrap();
    zip.start_file("mod.wasm", opts).unwrap();
    zip.write_all(b"legacy").unwrap();
    zip.finish().unwrap();

    let verdict = verify::hnmod::validate(&path, None, None);
    assert!(!verdict.pass, "foreign top-level entries must fail");
    assert!(verdict
        .issues
        .iter()
        .any(|i| i.code == "top_level_entry" && i.message.contains("mod.wasm")));

    let _ = std::fs::remove_dir_all(&tmp);
}
