//! Real upstream Java sources, fetched from the mod's GitHub repository via
//! the Modrinth metadata (spec §7 modpack inputs). When the source of a
//! feature's class is available, the LLM fallback prompt carries the actual
//! upstream code instead of the bare class/method inventory the class-file
//! AST can offer.

use std::collections::BTreeMap;
use std::path::Path;

/// Internal class name (`a/b/c/Class`) -> upstream `.java` source text.
pub struct JavaSources {
    /// GitHub `owner/repo` the sources were fetched from, when known.
    repo: Option<String>,
    files: BTreeMap<String, String>,
}

impl JavaSources {
    pub fn empty() -> Self {
        JavaSources {
            repo: None,
            files: BTreeMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn repo(&self) -> Option<&str> {
        self.repo.as_deref()
    }

    pub fn set_repo(&mut self, repo: impl Into<String>) {
        self.repo = Some(repo.into());
    }

    /// Walk a staged source tree and index every `.java` file by its fully
    /// qualified internal name. The class name comes from the `package`
    /// declaration plus the file stem when present, otherwise from the
    /// path relative to the source root — real repositories keep those two
    /// in agreement, and where they don't, the package declaration wins.
    pub fn load(dir: &Path) -> Self {
        let mut files = BTreeMap::new();
        walk_java(dir, dir, &mut files);
        JavaSources { repo: None, files }
    }

    /// The upstream source of one class (matched in path form `a/b/c/Name`
    /// or dotted form `a.b.c.Name`; a leading `Outer$Inner` suffix falls
    /// back to the top-level file), truncated to `max_chars`.
    pub fn snippet_for(&self, class: &str, max_chars: usize) -> Option<String> {
        let mut key = class.trim().replace('.', "/");
        if !self.files.contains_key(&key) {
            // inner classes live in the top-level file `Outer.java`
            if let Some(top) = key.split('$').next() {
                key = top.to_string();
            }
        }
        let text = self.files.get(&key)?;
        Some(truncate_chars(text, max_chars))
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}\n// … truncated upstream source")
}

fn walk_java(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            walk_java(root, &path, out);
        } else if path.extension().map(|x| x == "java").unwrap_or(false) {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let key = class_key(root, &path, &text);
            out.insert(key, text);
        }
    }
}

/// Internal name for a source file: `package a.b.c;` + file stem, falling
/// back to the path under the source root when no package is declared.
fn class_key(root: &Path, path: &Path, text: &str) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let package = text.lines().find_map(|line| {
        let line = line.trim();
        let rest = line.strip_prefix("package ")?;
        let rest = rest.trim_end_matches(';').trim();
        (!rest.is_empty()).then(|| rest.to_string())
    });
    match package {
        Some(pkg) => format!("{}/{}", pkg.replace('.', "/"), stem),
        None => {
            let rel = path
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| stem.clone());
            rel.trim_end_matches(".java").to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_by_package_declaration_and_truncates() {
        let tmp = std::env::temp_dir().join(format!("ap-sources-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let src = tmp.join("src/main/java/net/example");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("Tick.java"),
            "package net.example;\n\npublic class Tick {\n    void onTick() {}\n}\n",
        )
        .unwrap();
        // a source file without a package declaration: falls back to the path
        std::fs::create_dir_all(tmp.join("gen")).unwrap();
        std::fs::write(tmp.join("gen/Odd.java"), "class Odd {}\n").unwrap();

        let js = JavaSources::load(&tmp);
        assert!(!js.is_empty());

        let long_code = js
            .snippet_for("net.example.Tick", 10_000)
            .expect("source found by package+stem");
        assert!(long_code.contains("onTick"), "real source text is carried");

        // dotted form and inner-class form both resolve
        assert!(js.snippet_for("net.example.Tick$Inner", 10_000).is_some());

        let cut = js.snippet_for("net.example.Tick", 5).expect("present");
        assert!(
            cut.contains("truncated"),
            "over-budget source is cut, got: {cut}"
        );

        assert!(js.snippet_for("net.example.Missing", 100).is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
