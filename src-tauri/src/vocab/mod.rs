//! Lightweight per-target project vocabulary. No vector database: just a
//! bounded list of identifiers gathered from manifests, file names and a few
//! declarations. Source code itself never leaves this module.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use crate::target::TargetSlot;

pub const EXCLUDED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".venv",
    "venv",
    "__pycache__",
    "vendor",
];

const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "py", "go", "java", "kt", "swift", "c", "h", "cc",
    "cpp", "hpp", "cs", "rb", "php", "vue", "svelte",
];

#[derive(Debug, Clone)]
pub struct ScanLimits {
    pub max_files: usize,
    pub max_depth: usize,
    pub max_file_bytes: u64,
    pub max_recent_files: usize,
    pub max_symbol_files: usize,
    pub max_symbols: usize,
    pub max_terms: usize,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_files: 3000,
            max_depth: 8,
            max_file_bytes: 256 * 1024,
            max_recent_files: 40,
            max_symbol_files: 40,
            max_symbols: 150,
            max_terms: 400,
        }
    }
}

struct Collector {
    seen: HashSet<String>,
    terms: Vec<String>,
    max: usize,
}

impl Collector {
    fn add(&mut self, term: &str) {
        let term = term.trim();
        if term.len() < 2 || term.len() > 64 || self.terms.len() >= self.max {
            return;
        }
        if self.seen.insert(term.to_string()) {
            self.terms.push(term.to_string());
        }
    }
}

fn read_small(path: &Path, max_bytes: u64) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > max_bytes {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.iter().take(1024).any(|b| *b == 0) {
        return None; // binary
    }
    String::from_utf8(bytes).ok()
}

fn scan_package_json(text: &str, c: &mut Collector) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    if let Some(name) = v.get("name").and_then(|n| n.as_str()) {
        c.add(name);
    }
    for key in ["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(deps) = v.get(key).and_then(|d| d.as_object()) {
            for name in deps.keys() {
                c.add(name);
            }
        }
    }
}

fn unquote(s: &str) -> &str {
    s.trim().trim_matches(|ch| ch == '"' || ch == '\'')
}

fn scan_cargo_toml(text: &str, c: &mut Collector) {
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line.trim_matches(|ch| ch == '[' || ch == ']').to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if section == "package" && key == "name" {
            c.add(unquote(value));
        } else if section.ends_with("dependencies") && !key.starts_with('#') {
            c.add(unquote(key));
        }
    }
}

fn leading_package_name(spec: &str) -> &str {
    let spec = unquote(spec.trim().trim_end_matches(','));
    let end = spec
        .find(|ch: char| !(ch.is_alphanumeric() || ch == '-' || ch == '_' || ch == '.'))
        .unwrap_or(spec.len());
    &spec[..end]
}

fn scan_pyproject(text: &str, c: &mut Collector) {
    let mut in_deps = false;
    for line in text.lines() {
        let line = line.trim();
        if in_deps {
            if line.starts_with(']') {
                in_deps = false;
            } else {
                c.add(leading_package_name(line));
            }
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            if key == "name" {
                c.add(unquote(value));
            } else if key == "dependencies" && value.trim_start().starts_with('[') {
                let inline = value.trim().trim_start_matches('[');
                if let Some(body) = inline.strip_suffix(']') {
                    for item in body.split(',') {
                        c.add(leading_package_name(item));
                    }
                } else {
                    in_deps = true;
                }
            }
        }
    }
}

fn scan_go_mod(text: &str, c: &mut Collector) {
    for line in text.lines() {
        let line = line.trim();
        let path = if let Some(rest) = line.strip_prefix("module ") {
            rest
        } else if let Some(rest) = line.strip_prefix("require ") {
            rest
        } else if line.contains('/') && !line.starts_with("//") {
            line
        } else {
            continue;
        };
        if let Some(module) = path.split_whitespace().next() {
            if let Some(last) = module.trim_matches('(').rsplit('/').next() {
                c.add(last);
            }
        }
    }
}

const DECL_KEYWORDS: &[&str] = &[
    "fn ",
    "function ",
    "class ",
    "interface ",
    "type ",
    "struct ",
    "enum ",
    "trait ",
    "def ",
    "func ",
];

fn scan_symbols(text: &str, c: &mut Collector, budget: &mut usize) {
    for line in text.lines() {
        if *budget == 0 {
            return;
        }
        let line = line.trim_start();
        for kw in DECL_KEYWORDS {
            let Some(pos) = line.find(kw) else { continue };
            // Keyword must start a token, not sit inside another word.
            if pos > 0
                && line[..pos]
                    .chars()
                    .last()
                    .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
            {
                continue;
            }
            let rest = &line[pos + kw.len()..];
            let ident: String = rest
                .chars()
                .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
                .collect();
            if ident.len() >= 3 && !ident.chars().next().unwrap().is_numeric() {
                let before = c.terms.len();
                c.add(&ident);
                if c.terms.len() > before {
                    *budget -= 1;
                }
            }
            break;
        }
    }
}

/// Scans `root` for vocabulary terms within `limits`.
pub fn scan_project(root: &Path, limits: &ScanLimits) -> Vec<String> {
    let mut c = Collector {
        seen: HashSet::new(),
        terms: Vec::new(),
        max: limits.max_terms,
    };
    if !root.is_dir() {
        return c.terms;
    }
    if let Some(name) = root.file_name() {
        c.add(&name.to_string_lossy());
    }

    type Manifest = (&'static str, fn(&str, &mut Collector));
    let manifests: [Manifest; 4] = [
        ("package.json", scan_package_json),
        ("Cargo.toml", scan_cargo_toml),
        ("pyproject.toml", scan_pyproject),
        ("go.mod", scan_go_mod),
    ];
    for (file, scan) in manifests {
        if let Some(text) = read_small(&root.join(file), limits.max_file_bytes) {
            scan(&text, &mut c);
        }
    }

    // Bounded walk collecting source files with their modification time.
    let mut sources: Vec<(SystemTime, PathBuf)> = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    let mut visited = 0usize;
    'walk: while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                if EXCLUDED_DIRS.contains(&name.as_str()) || name.starts_with('.') {
                    continue;
                }
                if depth < limits.max_depth {
                    stack.push((entry.path(), depth + 1));
                }
                continue;
            }
            visited += 1;
            if visited > limits.max_files {
                break 'walk;
            }
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if SOURCE_EXTENSIONS.contains(&ext) {
                let mtime = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                sources.push((mtime, path));
            }
        }
    }

    // "Recently opened" is approximated by most recently modified.
    sources.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    for (_, path) in sources.iter().take(limits.max_recent_files) {
        if let Some(name) = path.file_name() {
            c.add(&name.to_string_lossy());
        }
        if let Some(stem) = path.file_stem() {
            c.add(&stem.to_string_lossy());
        }
    }
    let mut budget = limits.max_symbols;
    for (_, path) in sources.iter().take(limits.max_symbol_files) {
        if budget == 0 {
            break;
        }
        if let Some(text) = read_small(path, limits.max_file_bytes) {
            scan_symbols(&text, &mut c, &mut budget);
        }
    }
    c.terms
}

/// Current branch name from `.git/HEAD`, if `root` is a git checkout on a branch.
pub fn git_branch(root: &Path) -> Option<String> {
    let head = std::fs::read_to_string(root.join(".git").join("HEAD")).ok()?;
    let branch = head.trim().strip_prefix("ref: refs/heads/")?;
    (!branch.is_empty() && branch.len() <= 80).then(|| branch.to_string())
}

/// Identifier-like words from a window title: file names and names with
/// inner capitals, digits, dots, dashes or underscores. Plain words such as
/// "Visual" or "Terminal" are left out so they do not bias recognition.
pub fn title_hints(title: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in title.split(|c: char| !(c.is_alphanumeric() || matches!(c, '.' | '_' | '-'))) {
        let token = token.trim_matches(|c| matches!(c, '.' | '-' | '_'));
        if token.len() < 3 || token.len() > 40 || !token.chars().any(|c| c.is_alphabetic()) {
            continue;
        }
        let identifier_like = token
            .chars()
            .any(|c| matches!(c, '.' | '_' | '-') || c.is_ascii_digit())
            || token.chars().skip(1).any(|c| c.is_uppercase());
        if identifier_like && !out.iter().any(|t| t == token) {
            out.push(token.to_string());
        }
        if out.len() >= 8 {
            break;
        }
    }
    out
}

/// Per-target vocabulary cache. Each target keeps its own term list, so
/// switching targets switches hotwords and correction context with it.
#[derive(Default)]
pub struct VocabStore {
    scanned: Mutex<HashMap<String, Vec<String>>>,
}

impl VocabStore {
    /// Rescans the target's project root (blocking; call off the UI thread).
    pub fn refresh(&self, target: &TargetSlot) -> usize {
        let terms = match &target.project_root {
            Some(root) => scan_project(Path::new(root), &ScanLimits::default()),
            None => Vec::new(),
        };
        let n = terms.len();
        self.scanned
            .lock()
            .unwrap()
            .insert(target.id.clone(), terms);
        n
    }

    pub fn forget(&self, target_id: &str) {
        self.scanned.lock().unwrap().remove(target_id);
    }

    /// Manual terms first, then scanned terms, without duplicates.
    pub fn terms_for(&self, target: &TargetSlot) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let scanned = self.scanned.lock().unwrap();
        let scanned_terms = scanned.get(&target.id).map(|v| v.as_slice()).unwrap_or(&[]);
        // Automatic hints, like a dictation tool that knows where you are:
        // the project name, the current git branch, and identifier-like
        // words from the window title (file names, CamelCase names).
        let mut hints: Vec<String> = Vec::new();
        if let Some(name) = &target.project_name {
            hints.push(name.clone());
        }
        if let Some(root) = &target.project_root {
            if let Some(branch) = git_branch(Path::new(root)) {
                hints.push(branch);
            }
        }
        hints.extend(title_hints(&target.title_hint));
        for t in target
            .manual_terms
            .iter()
            .chain(&hints)
            .chain(scanned_terms)
        {
            if seen.insert(t.clone()) {
                out.push(t.clone());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::mock::MockDesktop;
    use crate::types::OutputKind;
    use std::fs;

    fn write(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    #[test]
    fn project_scanning_excludes_dependency_and_build_directories() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("my-app");
        write(
            &root,
            "package.json",
            r#"{"name":"my-app","dependencies":{"@tanstack/react-query":"5"}}"#,
        );
        write(
            &root,
            "src/hooks/useUserQuery.ts",
            "export function useUserQuery(userId: string) {}\n",
        );
        write(
            &root,
            "node_modules/leftpad/secretNodeModule.js",
            "function secretNodeFn() {}\n",
        );
        write(
            &root,
            ".git/hooks/secretGitHook.js",
            "function secretGitFn() {}\n",
        );
        write(
            &root,
            "target/debug/secretTargetFile.rs",
            "fn secret_target_fn() {}\n",
        );
        write(
            &root,
            "dist/secretDistBundle.js",
            "function secretDistFn() {}\n",
        );
        write(
            &root,
            "build/secretBuild.js",
            "function secretBuildFn() {}\n",
        );
        write(&root, ".next/secretNext.js", "function secretNextFn() {}\n");

        let terms = scan_project(&root, &ScanLimits::default());
        assert!(terms.contains(&"my-app".to_string()));
        assert!(terms.contains(&"@tanstack/react-query".to_string()));
        assert!(terms.contains(&"useUserQuery".to_string()));
        assert!(terms.contains(&"useUserQuery.ts".to_string()));
        for t in &terms {
            assert!(
                !t.to_lowercase().contains("secret"),
                "leaked excluded term {t}"
            );
        }
    }

    #[test]
    fn manifests_and_limits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "Cargo.toml", "[package]\nname = \"voice-core\"\n[dependencies]\ntokio = \"1\"\nserde = { version = \"1\" }\n");
        write(
            root,
            "pyproject.toml",
            "[project]\nname = \"pytool\"\ndependencies = [\n  \"requests>=2\",\n  \"numpy\",\n]\n",
        );
        write(
            root,
            "go.mod",
            "module example.com/acme/gateway\nrequire github.com/gin-gonic/gin v1.9.0\n",
        );
        write(root, "blob.rs", "fn hidden_in_binary() {}\0\0");
        write(
            root,
            "big.rs",
            &format!("fn huge_function() {{}}\n{}", "// pad\n".repeat(60_000)),
        );
        let terms = scan_project(root, &ScanLimits::default());
        for want in [
            "voice-core",
            "tokio",
            "serde",
            "pytool",
            "requests",
            "numpy",
            "gateway",
            "gin",
        ] {
            assert!(
                terms.contains(&want.to_string()),
                "missing {want}: {terms:?}"
            );
        }
        assert!(
            !terms.contains(&"hidden_in_binary".to_string()),
            "binary files are skipped"
        );
        assert!(
            !terms.contains(&"huge_function".to_string()),
            "oversized files are skipped"
        );

        let limited = scan_project(
            root,
            &ScanLimits {
                max_terms: 3,
                ..Default::default()
            },
        );
        assert_eq!(limited.len(), 3);
    }

    #[test]
    fn each_target_has_its_own_vocabulary() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("api"),
            "server.go",
            "func HandleLogin() {}\n",
        );
        write(
            &dir.path().join("web"),
            "App.tsx",
            "function renderDashboard() {}\n",
        );
        let mut a = TargetSlot::from_window(
            &MockDesktop::window("1", 1, "x", "a"),
            1,
            OutputKind::Prompt,
        );
        let mut b = TargetSlot::from_window(
            &MockDesktop::window("2", 2, "x", "b"),
            2,
            OutputKind::Prompt,
        );
        a.project_root = Some(dir.path().join("api").to_string_lossy().to_string());
        b.project_root = Some(dir.path().join("web").to_string_lossy().to_string());
        a.manual_terms = vec!["JWT".into()];
        let store = VocabStore::default();
        store.refresh(&a);
        store.refresh(&b);
        let ta = store.terms_for(&a);
        let tb = store.terms_for(&b);
        assert_eq!(ta[0], "JWT");
        assert!(
            ta.contains(&"HandleLogin".to_string()) && !ta.contains(&"renderDashboard".to_string())
        );
        assert!(
            tb.contains(&"renderDashboard".to_string()) && !tb.contains(&"HandleLogin".to_string())
        );
    }

    #[test]
    fn automatic_hints_from_title_project_and_branch() {
        assert_eq!(
            title_hints("● useUserQuery.ts — voiceBridge — Visual Studio Code"),
            vec!["useUserQuery.ts", "voiceBridge"]
        );
        assert!(title_hints("Terminal — zsh — 80×24").is_empty());
        assert_eq!(
            title_hints("api-server: feature_login (HEAD)"),
            vec!["api-server", "feature_login", "HEAD"]
        );

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("payments");
        write(&root, ".git/HEAD", "ref: refs/heads/fix/login-timeout\n");
        assert_eq!(git_branch(&root).as_deref(), Some("fix/login-timeout"));
        write(&root, ".git/HEAD", "3f2c1a9d\n");
        assert_eq!(git_branch(&root), None, "detached HEAD has no branch name");
        write(&root, ".git/HEAD", "ref: refs/heads/fix/login-timeout\n");

        let mut t = TargetSlot::from_window(
            &MockDesktop::window("1", 1, "x", "checkout.rs — payments"),
            1,
            OutputKind::Prompt,
        );
        t.project_root = Some(root.to_string_lossy().to_string());
        t.project_name = Some("payments".into());
        t.manual_terms = vec!["Stripe".into()];
        let terms = VocabStore::default().terms_for(&t);
        assert_eq!(
            terms,
            vec!["Stripe", "payments", "fix/login-timeout", "checkout.rs"]
        );
    }
}
