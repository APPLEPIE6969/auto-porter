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
fn llm_retries_through_nvidia_degraded_400s() {
    // The NVIDIA NIM gateway answers HTTP 400 "DEGRADED function cannot be
    // invoked" while the deployment behind a model id is degraded (observed
    // in CI on 2026-11-03). That is a server-side outage, so the fallback
    // must ride it out: two degraded 400s, then success — the tick feature
    // (no template) must end PORTED via the LLM path, not as llm_failed.
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let requests = Arc::new(AtomicUsize::new(0));
    let served = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf);
            let n = served.fetch_add(1, Ordering::SeqCst) + 1;
            let (status, body) = if n <= 2 {
                (
                    "400 Bad Request",
                    r#"{"status":400,"title":"Bad Request","detail":"Function id 'eb8265c4-e47d-4047-918d-c12746cc2cee': DEGRADED function cannot be invoked"}"#,
                )
            } else {
                (
                    "200 OK",
                    r#"{"choices":[{"message":{"content":"/// LLM-generated tick hook.\nfn on_player_tick() {}"}}]}"#,
                )
            };
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });

    let jar = ws("tests/fixtures/jars/chunkcourier-neoforge-2.0.0.jar");
    let tmp = std::env::temp_dir().join(format!("ap-llm-degraded-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);

    std::env::set_var("OPENAI_API_KEY", "test-key-for-degraded-e2e");
    let mut cfg = PorterConfig::default();
    cfg.llm.provider = "openai".to_string();
    cfg.llm.base_url = Some(format!("http://{addr}/v1/chat/completions"));
    cfg.llm.retry_delay_ms = 1; // keep the test fast
    let out_dir = tmp.join("chunkcourier");
    let opts = PortOptions {
        horizon_revision: None,
        data_only: false,
        output: out_dir.clone(),
        verbose: false,
        hooks_json: None,
    };

    let ported = port(&jar, &cfg, &opts);
    let made = requests.load(Ordering::SeqCst);
    std::env::remove_var("OPENAI_API_KEY");

    let ported = ported.expect("port succeeds after riding out the degraded window");
    let tick: Vec<_> = ported
        .report
        .features
        .iter()
        .filter(|f| f.feature_id.contains("playertickevent"))
        .collect();
    assert_eq!(tick.len(), 1, "tick feature present in report");
    assert_eq!(
        tick[0].status,
        shared::report::FeatureStatus::Ported,
        "tick hook ported through the LLM fallback"
    );
    // spec §9: every failure is recorded — the two retried attempts stay in
    // the report history even though the third attempt recovered, so assert
    // the degraded entries exist AND the feature still ended PORTED.
    assert_eq!(
        ported
            .report
            .errors
            .iter()
            .filter(|e| e.message.contains("DEGRADED"))
            .count(),
        2,
        "both degraded-400 attempts recorded honestly"
    );
    assert_eq!(
        made, 3,
        "two degraded 400s retried, third request generated the hook"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Serve the fixture jar bytes for the synthetic .mrpack download tests.
fn serve_fixture_jar(
    jar_bytes: Vec<u8>,
) -> (
    std::net::SocketAddr,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let hits = Arc::new(AtomicUsize::new(0));
    let served = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            served.fetch_add(1, Ordering::SeqCst);
            let req = String::from_utf8_lossy(&buf);
            let (status, body): (&str, Vec<u8>) = if req.starts_with("GET /veinminer.jar") {
                ("200 OK", jar_bytes.clone())
            } else {
                ("404 Not Found", b"not found".to_vec())
            };
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/java-archive\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    (addr, hits)
}

fn sha1_hex_of(bytes: &[u8]) -> String {
    use sha1::Digest;
    sha1::Sha1::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Write an in-memory zip to a temp file with the given name.
fn write_zip(path: &std::path::Path, entries: &[(&str, Vec<u8>)]) {
    use std::io::Write as _;
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
    for (name, bytes) in entries {
        zip.start_file(*name, opts).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn mrpack_manifest_mods_are_downloaded_sha1_verified_and_ported() {
    // Spec §7: port-pack accepts a Modrinth .mrpack. Offline of the real
    // carrier, the manifest-driven fallback must enumerate the mods the
    // index lists, download them from the pinned URLs, verify the pinned
    // sha1, stage the jar, and run each through the full pipeline.
    let fixture = ws("tests/fixtures/jars/veinminer-fabric-1.0.0.jar");
    let jar_bytes = std::fs::read(&fixture).expect("fixture jar");
    let real_sha1 = sha1_hex_of(&jar_bytes);
    let (addr, hits) = serve_fixture_jar(jar_bytes.clone());

    let index = serde_json::json!({
        "formatVersion": 1,
        "gameVersion": "1.21.1",
        "name": "e2e-pack",
        "summary": "synthetic pack for the manifest-driven fallback test",
        "dependencies": {"minecraft": "1.21.1", "fabric-loader": "0.16.9"},
        "files": [
            {
                "path": "mods/veinminer.jar",
                "hashes": {"sha1": real_sha1, "sha512": "not-checked-here"},
                "env": {"client": "required", "server": "required"},
                "fileSize": jar_bytes.len(),
                "downloads": [format!("http://{addr}/veinminer.jar")]
            },
            {
                "path": "resourcepacks/Textures.zip",
                "hashes": {"sha1": "deadbeef"},
                "fileSize": 4,
                "downloads": [format!("http://{addr}/never-fetched.zip")]
            }
        ]
    });
    let tmp = std::env::temp_dir().join(format!("ap-mrpack-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let pack_path = tmp.join("e2e-pack.mrpack");
    write_zip(
        &pack_path,
        &[
            ("modrinth.index.json", serde_json::to_vec(&index).unwrap()),
            ("overrides/config/option.txt", b"enable=true\n".to_vec()),
            (
                "client-overrides/shaders/pack.fsh",
                b"#version 150\n".to_vec(),
            ),
            (
                "server-overrides/serverconfig.toml",
                b"[veinminer]\non = true\n".to_vec(),
            ),
        ],
    );

    let cfg = PorterConfig::default();
    let out_dir = tmp.join("out");
    let opts = PortOptions {
        horizon_revision: None,
        data_only: false,
        output: out_dir.clone(),
        verbose: false,
        hooks_json: None,
    };
    let summary = auto_porter_lib::port_pack(&pack_path, &cfg, &opts, true).expect("port-pack");
    let made = hits.load(std::sync::atomic::Ordering::SeqCst);

    let entry = &summary["results"][0];
    assert_eq!(entry["archive"], "e2e-pack");
    assert_eq!(entry["format"], "mrpack");
    let mods = entry["mods"].as_array().expect("mods listed");
    assert_eq!(
        mods.len(),
        2,
        "the code mod and the resource pack both listed"
    );
    let code_mod = mods
        .iter()
        .find(|m| m["mod"] == "mods/veinminer.jar")
        .expect("code mod entry");
    assert_eq!(code_mod["mod_id"], "veinminer");
    assert_eq!(code_mod["sha1"], "verified", "pinned sha1 checked and ok");
    // a resource pack is data for the launcher import, not a code mod — it
    // must be recorded with its kind and never pushed through the pipeline
    let pack_entry = mods
        .iter()
        .find(|m| m["mod"] == "resourcepacks/Textures.zip")
        .expect("resource pack entry");
    assert_eq!(pack_entry["status"], "resource-pack");
    assert_eq!(summary["ported"], 1);
    assert_eq!(summary["failures"], 0);
    assert_eq!(made, 1, "only the code mod is downloaded");

    // the staged jar and the pipeline artifacts land in the pack's work dir
    assert!(out_dir.join("e2e-pack/mods/veinminer.jar").is_file());
    assert!(out_dir.join("e2e-pack/veinminer.hmod").is_file());
    assert!(out_dir.join("e2e-pack/veinminer-report.json").is_file());
    // all three override roots are carried as data
    let carried = entry["overrides_carried"].as_object().unwrap();
    assert_eq!(carried.get("overrides"), Some(&serde_json::json!(1)));
    assert_eq!(carried.get("client-overrides"), Some(&serde_json::json!(1)));
    assert_eq!(carried.get("server-overrides"), Some(&serde_json::json!(1)));
    assert!(out_dir.join("e2e-pack/config/option.txt").is_file());
    assert!(out_dir.join("e2e-pack/shaders/pack.fsh").is_file());
    assert!(out_dir.join("e2e-pack/serverconfig.toml").is_file());

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn mrpack_sha1_mismatch_is_recorded_and_not_ported() {
    let fixture = ws("tests/fixtures/jars/veinminer-fabric-1.0.0.jar");
    let jar_bytes = std::fs::read(&fixture).expect("fixture jar");
    let real_len = jar_bytes.len();
    let (addr, hits) = serve_fixture_jar(jar_bytes);

    let index = serde_json::json!({
        "formatVersion": 1,
        "name": "bad-hash-pack",
        "dependencies": {"minecraft": "1.21.1", "fabric-loader": "0.16.9"},
        "files": [{
            "path": "mods/veinminer.jar",
            "hashes": {"sha1": "0000000000000000000000000000000000000000"},
            "fileSize": real_len,
            "downloads": [format!("http://{addr}/veinminer.jar")]
        }]
    });
    let tmp = std::env::temp_dir().join(format!("ap-mrpack-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let pack_path = tmp.join("bad-hash-pack.mrpack");
    write_zip(
        &pack_path,
        &[("modrinth.index.json", serde_json::to_vec(&index).unwrap())],
    );

    let cfg = PorterConfig::default();
    let out_dir = tmp.join("out");
    let opts = PortOptions {
        horizon_revision: None,
        data_only: false,
        output: out_dir.clone(),
        verbose: false,
        hooks_json: None,
    };
    let summary =
        auto_porter_lib::port_pack(&pack_path, &cfg, &opts, true).expect("port-pack runs");
    let made = hits.load(std::sync::atomic::Ordering::SeqCst);

    assert_eq!(summary["ported"], 0);
    assert_eq!(summary["failures"], 1);
    let entry = &summary["results"][0];
    let mods = entry["mods"].as_array().unwrap();
    assert_eq!(mods.len(), 1);
    let err = mods[0]["error"].as_str().unwrap();
    assert!(err.contains("sha1 mismatch"), "honest hash failure: {err}");
    assert_eq!(made, 1);
    assert!(
        !out_dir.join("bad-hash-pack/veinminer.hmod").exists(),
        "a file failing its pinned hash is never handed to the pipeline"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn curseforge_manifest_mods_are_recorded_honestly() {
    // CurseForge manifests name files only as (projectID, fileID); without
    // the key-gated CF API they cannot be downloaded, so the fallback must
    // record each pair as requires-curseforge-api — never guess a URL.
    let manifest = serde_json::json!({
        "minecraft": {
            "version": "1.21.1",
            "modLoaders": [{"id": "fabric-0.16.9", "primary": true}]
        },
        "name": "cf-pack",
        "files": [
            {"projectID": 111111, "fileID": 222222},
            {"projectID": 333333, "fileID": 444444}
        ],
        "overrides": "overrides"
    });
    let tmp = std::env::temp_dir().join(format!("ap-cfpack-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let pack_path = tmp.join("cf-pack.zip");
    write_zip(
        &pack_path,
        &[
            ("manifest.json", serde_json::to_vec(&manifest).unwrap()),
            ("overrides/config/common.toml", b"[a]\nb = 1\n".to_vec()),
        ],
    );

    let cfg = PorterConfig::default();
    let out_dir = tmp.join("out");
    let opts = PortOptions {
        horizon_revision: None,
        data_only: false,
        output: out_dir.clone(),
        verbose: false,
        hooks_json: None,
    };
    let summary =
        auto_porter_lib::port_pack(&pack_path, &cfg, &opts, true).expect("port-pack runs");

    let entry = &summary["results"][0];
    assert_eq!(entry["archive"], "cf-pack");
    assert_eq!(entry["format"], "curseforge");
    let mods = entry["mods"].as_array().unwrap();
    assert_eq!(mods.len(), 2);
    for m in mods {
        assert_eq!(m["status"], "requires-curseforge-api");
    }
    assert_eq!(mods[0]["project_id"], 111111);
    assert_eq!(mods[1]["file_id"], 444444);
    assert_eq!(summary["ported"], 0, "nothing invented, nothing faked");
    assert_eq!(
        summary["failures"], 0,
        "a key-gated source is not a failure"
    );
    assert!(out_dir.join("cf-pack/config/common.toml").is_file());

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
