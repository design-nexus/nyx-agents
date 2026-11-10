//! Builds libghostty-vt as a static library and generates its bindings.
//!
//! The source and dependency tarballs come from `vendor/libghostty-vt` (fetched
//! by `tools/fetch-libghostty-vt.sh`) and are checked against SHA256SUMS before
//! use. The build is offline: Zig reads its packages from the dependency
//! tarball and starts from an empty global cache, as PACKAGING-LIBGHOSTTY.md
//! requires for Zig 0.17.
//!
//! `NYX_GHOSTTY_VT_PREFIX` points at an existing install (with `lib/libghostty-vt.a`
//! and `include/ghostty/vt.h`) instead, for packagers that build it separately.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

const VERSION: &str = "1.0.0";

fn main() {
    println!("cargo:rerun-if-env-changed=NYX_GHOSTTY_VT_PREFIX");
    println!("cargo:rerun-if-env-changed=NYX_GHOSTTY_VT_VENDOR");
    println!("cargo:rerun-if-env-changed=ZIG");

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let prefix = match env::var_os("NYX_GHOSTTY_VT_PREFIX") {
        Some(p) => PathBuf::from(p),
        None => build_vendored(&out),
    };

    let lib = prefix.join("lib");
    let include = prefix.join("include");
    assert!(lib.join("libghostty-vt.a").is_file(), "no libghostty-vt.a in {}", lib.display());
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    println!("cargo:include={}", include.display());

    let bindings = bindgen::Builder::default()
        .header(include.join("ghostty/vt.h").to_string_lossy())
        .clang_arg(format!("-I{}", include.display()))
        .allowlist_function("ghostty_.*")
        .allowlist_type("Ghostty.*")
        .allowlist_var("GHOSTTY_.*")
        .prepend_enum_name(false)
        .default_enum_style(bindgen::EnumVariation::Consts)
        .derive_default(true)
        .layout_tests(false)
        .generate()
        .expect("generate libghostty-vt bindings");
    bindings.write_to_file(out.join("bindings.rs")).expect("write bindings");
}

fn build_vendored(out: &Path) -> PathBuf {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let vendor =
        env::var_os("NYX_GHOSTTY_VT_VENDOR").map(PathBuf::from).unwrap_or_else(|| manifest.join("../../vendor/libghostty-vt"));
    let src_tar = vendor.join(format!("libghostty-vt-{VERSION}.tar.gz"));
    let deps_tar = vendor.join(format!("libghostty-vt-{VERSION}-deps.tar.gz"));
    let sums = vendor.join("SHA256SUMS");
    for p in [&src_tar, &deps_tar, &sums] {
        println!("cargo:rerun-if-changed={}", p.display());
        if !p.is_file() {
            panic!("{} is missing; run tools/fetch-libghostty-vt.sh first", p.display());
        }
    }
    let sums = fs::read_to_string(&sums).unwrap();
    for tar in [&src_tar, &deps_tar] {
        verify(tar, &sums);
    }

    let prefix = out.join("install");
    let stamp = prefix.join(format!(".built-{VERSION}"));
    if stamp.is_file() {
        return prefix;
    }

    let work = out.join("src");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).unwrap();
    for tar in [&src_tar, &deps_tar] {
        run(Command::new("tar").arg("-xzf").arg(tar).arg("-C").arg(&work));
    }

    // Zig 0.17 can crash on a global cache that already holds a package
    // archive, so every build starts from an empty one.
    let cache = out.join("zig-global-cache");
    let _ = fs::remove_dir_all(&cache);
    let _ = fs::remove_dir_all(&prefix);
    let zig = env::var("ZIG").unwrap_or_else(|_| "zig".into());
    run(Command::new(zig)
        .current_dir(work.join(format!("libghostty-vt-{VERSION}")))
        .env("ZIG_GLOBAL_CACHE_DIR", &cache)
        .arg("build")
        .arg("--prefix")
        .arg(&prefix)
        .arg("--system")
        .arg(work.join(format!("libghostty-vt-{VERSION}-deps")))
        .arg("-Doptimize=ReleaseFast")
        .arg("-Dcpu=baseline"));
    let _ = fs::remove_dir_all(&cache);
    fs::write(&stamp, VERSION).unwrap();
    prefix
}

fn verify(path: &Path, sums: &str) {
    let name = path.file_name().unwrap().to_string_lossy();
    let want = sums
        .lines()
        .filter_map(|l| l.split_once("  "))
        .find(|(_, f)| *f == name)
        .map(|(h, _)| h.to_string())
        .unwrap_or_else(|| panic!("{name} is not in SHA256SUMS"));
    let got = Sha256::digest(fs::read(path).unwrap());
    let got: String = got.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(got, want, "{name} does not match SHA256SUMS");
}

fn run(cmd: &mut Command) {
    let status = cmd.status().unwrap_or_else(|e| panic!("running {cmd:?}: {e}"));
    assert!(status.success(), "{cmd:?} failed with {status}");
}
