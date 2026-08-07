use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

/// cargo build 时自动构建前端：
/// - frontend/src 比 dist 新才执行 pnpm build
/// - B2T_SKIP_FRONTEND_BUILD=1 跳过（Docker/CI 中前端已单独构建）
/// - 无 pnpm 且 dist 不存在时生成占位页，保证 rust-embed 可编译
fn main() {
    println!("cargo:rerun-if-changed=frontend/src");
    println!("cargo:rerun-if-changed=frontend/package.json");
    println!("cargo:rerun-if-changed=frontend/astro.config.mjs");
    println!("cargo:rerun-if-env-changed=B2T_SKIP_FRONTEND_BUILD");

    if env::var_os("B2T_SKIP_FRONTEND_BUILD").is_some() {
        return;
    }

    let dist_index = Path::new("frontend/dist/index.html");
    let stale = match (dist_index.exists(), newest_mtime(Path::new("frontend/src"))) {
        (true, Some(src)) => dist_index
            .metadata()
            .and_then(|m| m.modified())
            .map(|dist| src > dist)
            .unwrap_or(true),
        (false, _) => true,
        (true, None) => false,
    };
    if !stale {
        return;
    }

    if !has_pnpm() {
        if !dist_index.exists() {
            fs::create_dir_all("frontend/dist").unwrap();
            fs::write(
                dist_index,
                "<!doctype html><meta charset=utf-8><title>B2T</title><p>frontend not built</p>",
            )
            .unwrap();
            println!("cargo:warning=pnpm 不可用，已生成前端占位页");
        } else {
            println!("cargo:warning=pnpm 不可用，沿用现有 frontend/dist");
        }
        return;
    }

    if !Path::new("frontend/node_modules").exists() {
        run("pnpm", &["--dir", "frontend", "install"]);
    }
    run("pnpm", &["--dir", "frontend", "build"]);
}

fn has_pnpm() -> bool {
    env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).any(|p| p.join("pnpm").is_file()))
        .unwrap_or(false)
}

fn run(cmd: &str, args: &[&str]) {
    let status = Command::new(cmd)
        .args(args)
        .status()
        .unwrap_or_else(|e| panic!("无法执行 {cmd}: {e}"));
    assert!(status.success(), "{cmd} {:?} 执行失败", args);
}

fn newest_mtime(dir: &Path) -> Option<SystemTime> {
    let mut newest: Option<SystemTime> = None;
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let mtime = if path.is_dir() {
            newest_mtime(&path)
        } else {
            path.metadata().and_then(|m| m.modified()).ok()
        };
        if let Some(t) = mtime {
            if newest.map(|n| t > n).unwrap_or(true) {
                newest = Some(t);
            }
        }
    }
    newest
}
