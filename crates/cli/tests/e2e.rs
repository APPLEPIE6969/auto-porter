//! End-to-end integration test: the full five-phase pipeline on the fixture
//! jars (spec §12 definition of done, offline-achievable subset).

use auto_porter_lib::{analyze, port, verify_dir, PortOptions};
use shared::config::PorterConfig;
use shared::report::{CheckStatus, Overall};
use std::path::{Path, PathBuf};

/// Tests that touch process-global env vars (OPENAI_API_KEY,
/// AUTO_PORTER_MODRINTH_API, ...) serialize on this — cargo runs tests in
/// parallel threads and set_var/remove_var races are UB-adjacent.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        pack_classes: Vec::new(),
        source_dir: None,
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
        pack_classes: Vec::new(),
        source_dir: None,
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

    let _env = ENV_LOCK.lock().unwrap();
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
        pack_classes: Vec::new(),
        source_dir: None,
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
        pack_classes: Vec::new(),
        source_dir: None,
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
        pack_classes: Vec::new(),
        source_dir: None,
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
        pack_classes: Vec::new(),
        source_dir: None,
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

/// Build a minimal Java class file (JVMS ch. 4) carrying one runtime-visible
/// class annotation: `@Mixin(targets = "<target>")`. Enough for the
/// decompose mixin extractor — no javac needed in the sandbox.
fn mixin_class_bytes(this: &str, target: &str) -> Vec<u8> {
    let utf8 = |s: &str| {
        let mut v = vec![1u8];
        v.extend_from_slice(&(s.len() as u16).to_be_bytes());
        v.extend_from_slice(s.as_bytes());
        v
    };
    let class = |idx: u16| {
        let mut v = vec![7u8];
        v.extend_from_slice(&idx.to_be_bytes());
        v
    };
    let cp: Vec<Vec<u8>> = vec![
        utf8(this),                                  // 1
        class(1),                                    // 2 this_class
        utf8("java/lang/Object"),                    // 3
        class(3),                                    // 4 super_class
        utf8("Lorg/spongepowered/asm/mixin/Mixin;"), // 5 annotation type
        utf8("targets"),                             // 6 element name
        utf8(target),                                // 7 element value
        utf8("RuntimeVisibleAnnotations"),           // 8 attribute name
    ];
    let mut out = Vec::new();
    out.extend_from_slice(&0xCAFEBABEu32.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes()); // minor
    out.extend_from_slice(&52u16.to_be_bytes()); // major
    out.extend_from_slice(&((cp.len() + 1) as u16).to_be_bytes());
    for entry in &cp {
        out.extend_from_slice(entry);
    }
    out.extend_from_slice(&0x0021u16.to_be_bytes()); // public super
    out.extend_from_slice(&2u16.to_be_bytes()); // this_class
    out.extend_from_slice(&4u16.to_be_bytes()); // super_class
    out.extend_from_slice(&0u16.to_be_bytes()); // interfaces
    out.extend_from_slice(&0u16.to_be_bytes()); // fields
    out.extend_from_slice(&0u16.to_be_bytes()); // methods
    out.extend_from_slice(&1u16.to_be_bytes()); // attributes
                                                // RuntimeVisibleAnnotations: 1 annotation, 1 element ('s' string value)
    let payload_len: u32 = 2 + 2 + 2 + 2 + 1 + 2;
    out.extend_from_slice(&8u16.to_be_bytes());
    out.extend_from_slice(&payload_len.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes()); // num annotations
    out.extend_from_slice(&5u16.to_be_bytes()); // type index
    out.extend_from_slice(&1u16.to_be_bytes()); // element pairs
    out.extend_from_slice(&6u16.to_be_bytes()); // "targets"
    out.push(b's');
    out.extend_from_slice(&7u16.to_be_bytes());
    out
}

#[test]
fn sibling_jars_resolve_cross_mod_mixin_targets() {
    // A compat mixin in jar A targets an API class that lives in jar B of
    // the same pack. Decomposing A alone must flag the target (spec §2
    // gate); the pack flow, which sees every sibling jar, must not.
    let tmp = std::env::temp_dir().join(format!("ap-pack-classes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let mixin_jar = tmp.join("compat-mod.jar");
    write_zip(
        &mixin_jar,
        &[(
            "com/test/CompatMixin.class",
            mixin_class_bytes("com/test/CompatMixin", "com/other/Foo"),
        )],
    );
    // the sibling jar that actually carries the targeted class
    let api_jar = tmp.join("api-mod.jar");
    write_zip(
        &api_jar,
        &[(
            "com/other/Foo.class",
            mixin_class_bytes("com/other/Foo", "com/other/Foo"),
        )],
    );
    // an LWJGL-targeting mixin: Minecraft always ships LWJGL on the runtime
    // classpath, so the target resolves even without a sibling jar
    let lwjgl_jar = tmp.join("input-mod.jar");
    write_zip(
        &lwjgl_jar,
        &[(
            "com/test/GlfwMixin.class",
            mixin_class_bytes("com/test/GlfwMixin", "org/lwjgl/glfw/GLFW"),
        )],
    );

    // decompose alone: com/other/Foo is unknown, org/lwjgl resolves
    let alone = decompose::decompose(&mixin_jar).expect("decompose");
    assert!(
        alone
            .gate_errors
            .iter()
            .any(|e| e.message.contains("com/other/Foo")),
        "single-jar decompose must flag the cross-mod target"
    );
    let lwjgl = decompose::decompose(&lwjgl_jar).expect("decompose");
    assert!(
        lwjgl.gate_errors.is_empty(),
        "org/lwjgl ships with Minecraft and must resolve: {:?}",
        lwjgl.gate_errors
    );
    // decompose_with the sibling class listed: resolves
    let with_pack = decompose::decompose_with(&mixin_jar, &["com/other/Foo".to_string()])
        .expect("decompose_with");
    assert!(
        with_pack.gate_errors.is_empty(),
        "the pack class index must resolve the target: {:?}",
        with_pack.gate_errors
    );

    // the pack flow itself: both jars together, no unknown-target errors
    let cfg = PorterConfig::default();
    let out_dir = tmp.join("out");
    let opts = PortOptions {
        horizon_revision: None,
        data_only: false,
        output: out_dir.clone(),
        verbose: false,
        hooks_json: None,
        pack_classes: Vec::new(),
        source_dir: None,
    };
    let summary = auto_porter_lib::port_pack(&tmp, &cfg, &opts, true).expect("port-pack");
    assert_eq!(summary["ported"], 3, "every jar in the dir ports");
    // mod ids are the sanitized jar stems
    let reports: Vec<serde_json::Value> = ["compat_mod", "input_mod"]
        .iter()
        .map(|id| {
            serde_json::from_str(
                &std::fs::read_to_string(out_dir.join(format!("{id}-report.json"))).unwrap(),
            )
            .unwrap()
        })
        .collect();
    for rep in &reports {
        assert!(
            !rep["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["error_type"] == "unknown_mixin_target"),
            "no false-positive mixin-target errors inside the pack: {}",
            rep["mod_id"]
        );
    }

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Serve canned responses by path prefix on a local port (Modrinth API /
/// codeload / CDN stand-ins for the source-fetch e2e).
fn serve_by_path(routes: Vec<(&'static str, Vec<u8>)>) -> std::net::SocketAddr {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let req = String::from_utf8_lossy(&buf);
            let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
            let hit = routes
                .iter()
                .find(|(prefix, _)| path.starts_with(prefix))
                .map(|(_, body)| body.clone())
                .unwrap_or_else(|| b"not found".to_vec());
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                hit.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.write_all(&hit);
        }
    });
    addr
}

/// One raw ustar header + data (tar::Builder refuses `../` names, and the
/// whole point of one entry is to be adversarial).
fn push_tar_entry(buf: &mut Vec<u8>, name: &str, data: &[u8]) {
    let mut h = [0u8; 512];
    h[..name.len()].copy_from_slice(name.as_bytes());
    h[100..108].copy_from_slice(b"0000644\0");
    h[108..116].copy_from_slice(b"0000000\0");
    h[116..124].copy_from_slice(b"0000000\0");
    h[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
    h[136..148].copy_from_slice(b"00000000000\0");
    h[156] = b'0'; // regular file
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    let sum: u32 = h.iter().map(|b| *b as u32).sum();
    h[148..156].copy_from_slice(format!("{:06o}\0 ", sum).as_bytes());
    buf.extend_from_slice(&h);
    buf.extend_from_slice(data);
    let pad = (512 - data.len() % 512) % 512;
    buf.extend(vec![0u8; pad]);
}

/// A gzipped tarball like codeload's default-branch archive (one `<repo>-<ref>/`
/// root), with the given text files.
fn gz_tar(entries: &[(&str, &str)]) -> Vec<u8> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use tar::Builder;
    type TarEntry<'a> = &'a (&'a str, &'a str);
    let (evil, normal): (Vec<TarEntry>, Vec<TarEntry>) =
        entries.iter().partition(|(p, _)| p.starts_with("../"));
    let enc = GzEncoder::new(Vec::new(), Compression::fast());
    let mut builder = Builder::new(enc);
    for (path, content) in normal {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, *path, content.as_bytes())
            .unwrap();
    }
    let mut inner = builder.into_inner().unwrap().finish().unwrap();
    for (path, content) in evil {
        push_tar_entry(&mut inner, path, content.as_bytes());
    }
    inner.extend_from_slice(&[0u8; 1024]); // two zero blocks terminate the archive
    inner
}

#[test]
fn mrpack_fetches_upstream_source_from_modrinth_linked_github() {
    // Spec §7 + the Modrinth page's own data: the CDN URL embeds the project
    // id, the public API carries `source_url`, and for GitHub repos the
    // default-branch tarball is fetchable without a key. The fetched source
    // must be staged under the pack work dir and every non-fetchable case
    // (no link, non-GitHub host) recorded honestly.
    let jar_one = std::fs::read(ws("tests/fixtures/jars/veinminer-fabric-1.0.0.jar")).unwrap();
    let jar_two = std::fs::read(ws("tests/fixtures/jars/torchtools-neoforge-0.3.1.jar")).unwrap();
    let jar_three = std::fs::read(ws("tests/fixtures/jars/moreassets-fabric-2.1.0.jar")).unwrap();
    let sha = |b: &[u8]| sha1_hex_of(b);

    let cdn = serve_by_path(vec![
        ("/data/AAAA0001/", jar_one.clone()),
        ("/data/AAAA0002/", jar_two.clone()),
        ("/data/AAAA0003/", jar_three.clone()),
    ]);
    let api = serve_by_path(vec![
        (
            "/v2/project/AAAA0001",
            br#"{"slug":"veinminer","source_url":"https://github.com/example/veinminer"}"#.to_vec(),
        ),
        (
            "/v2/project/AAAA0002",
            br#"{"slug":"torchtools","source_url":null}"#.to_vec(),
        ),
        (
            "/v2/project/AAAA0003",
            br#"{"slug":"moreassets","source_url":"https://gitlab.com/other/assets"}"#.to_vec(),
        ),
    ]);
    let tarball = gz_tar(&[
        ("veinminer-main/src/main/java/net/example/Tick.java",
         "package net.example;\n\npublic class Tick {\n    // real upstream behaviour\n    void onTick() { keep(); }\n    void keep() {}\n}\n"),
        ("veinminer-main/README.md", "docs\n"),
        ("../evil.txt", "must never escape"),
    ]);
    let gh = serve_by_path(vec![("/example/veinminer/tar.gz/HEAD", tarball)]);

    let _env = ENV_LOCK.lock().unwrap();
    std::env::set_var("AUTO_PORTER_MODRINTH_API", format!("http://{api}"));
    std::env::set_var("AUTO_PORTER_GITHUB_CODELOAD", format!("http://{gh}"));

    let index = serde_json::json!({
        "formatVersion": 1,
        "gameVersion": "1.21.1",
        "name": "e2e-src",
        "files": [
            {
                "path": "mods/one.jar",
                "hashes": {"sha1": sha(&jar_one)},
                "fileSize": jar_one.len(),
                "downloads": [format!("http://{cdn}/data/AAAA0001/versions/wip/one.jar")]
            },
            {
                "path": "mods/two.jar",
                "hashes": {"sha1": sha(&jar_two)},
                "fileSize": jar_two.len(),
                "downloads": [format!("http://{cdn}/data/AAAA0002/versions/wip/two.jar")]
            },
            {
                "path": "mods/three.jar",
                "hashes": {"sha1": sha(&jar_three)},
                "fileSize": jar_three.len(),
                "downloads": [format!("http://{cdn}/data/AAAA0003/versions/wip/three.jar")]
            }
        ]
    });
    let tmp = std::env::temp_dir().join(format!("ap-src-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let pack_path = tmp.join("e2e-src.mrpack");
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
        pack_classes: Vec::new(),
        source_dir: None,
    };
    let summary = auto_porter_lib::port_pack(&pack_path, &cfg, &opts, true).expect("port-pack");
    std::env::remove_var("AUTO_PORTER_MODRINTH_API");
    std::env::remove_var("AUTO_PORTER_GITHUB_CODELOAD");

    let mods = summary["results"][0]["mods"].as_array().unwrap();
    assert_eq!(mods.len(), 3, "every code mod listed");

    let one = mods.iter().find(|m| m["mod"] == "mods/one.jar").unwrap();
    assert_eq!(one["code_source"]["status"], "fetched");
    assert_eq!(one["code_source"]["repo"], "example/veinminer");
    assert_eq!(one["mod_id"], "veinminer", "the mod still ports");
    // the source tree is staged (flattened out of the codeload root)
    let staged = out_dir.join("e2e-src/sources/one");
    assert!(
        staged.join("src/main/java/net/example/Tick.java").is_file(),
        "real source staged under the pack work dir"
    );
    assert!(
        !tmp.join("evil.txt").exists(),
        "a `../` tar entry must never escape the staging dir"
    );
    // and it is real, indexed upstream source the LLM prompt can use
    let sources = port_code::sources::JavaSources::load(&staged);
    let snippet = sources
        .snippet_for("net.example.Tick", 10_000)
        .expect("source indexed by package + stem");
    assert!(snippet.contains("onTick"));
    assert!(snippet.contains("real upstream behaviour"));

    let two = mods.iter().find(|m| m["mod"] == "mods/two.jar").unwrap();
    assert_eq!(two["code_source"]["status"], "no-source-url");
    let three = mods.iter().find(|m| m["mod"] == "mods/three.jar").unwrap();
    assert_eq!(three["code_source"]["status"], "non-github");
    assert_eq!(
        three["code_source"]["repo"],
        "https://gitlab.com/other/assets"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn llm_prompt_carries_real_upstream_source() {
    // The point of the source fetch: when the mod's GitHub source is staged,
    // the LLM fallback request must carry the actual upstream Java code (not
    // just the class/method inventory the class-file AST can produce).
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let _env = ENV_LOCK.lock().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let captured = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let store = captured.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = vec![0u8; 65536];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            if let Some(body) = req.split("\r\n\r\n").nth(1) {
                *store.lock().unwrap() = body.to_string();
            }
            let body = r#"{"choices":[{"message":{"content":"/// ported with upstream source\fn on_player_tick() {}"}}]}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });

    // stage a real source tree matching the fixture's tick class
    let tmp = std::env::temp_dir().join(format!("ap-llm-src-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let src = tmp.join("src/main/java/com/example/chunkcourier");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("ChunkCourier.java"),
        "package com.example.chunkcourier;\n\npublic class ChunkCourier {\n    // UPSTREAM_ON_TICK_MARKER_42\n    void onPlayerTick() { sendChunks(); }\n}\n",
    )
    .unwrap();

    let jar = ws("tests/fixtures/jars/chunkcourier-neoforge-2.0.0.jar");
    std::env::set_var("OPENAI_API_KEY", "test-key-for-source-prompt");
    let mut cfg = PorterConfig::default();
    cfg.llm.provider = "openai".to_string();
    cfg.llm.base_url = Some(format!("http://{addr}/v1/chat/completions"));
    let out_dir = tmp.join("out");
    let opts = PortOptions {
        horizon_revision: None,
        data_only: false,
        output: out_dir,
        verbose: false,
        hooks_json: None,
        pack_classes: Vec::new(),
        source_dir: Some(tmp.clone()),
    };
    let ported = port(&jar, &cfg, &opts);
    std::env::remove_var("OPENAI_API_KEY");

    let ported = ported.expect("port succeeds");
    let tick: Vec<_> = ported
        .report
        .features
        .iter()
        .filter(|f| f.feature_id.contains("playertickevent"))
        .collect();
    assert_eq!(tick.len(), 1);
    assert_eq!(
        tick[0].status,
        shared::report::FeatureStatus::Ported,
        "tick feature ported through the source-aware LLM path"
    );
    let body = captured.lock().unwrap().clone();
    assert!(
        body.contains("UPSTREAM_ON_TICK_MARKER_42"),
        "the LLM request must carry the real upstream source, got: {body:.500}"
    );
    assert!(
        body.contains("real upstream source of com/example/chunkcourier/ChunkCourier"),
        "the snippet is attributed to its class"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
