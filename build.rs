use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let bridge_source = Path::new(&manifest_dir).join("scripts/code-tour-bridge.mjs");
    let bundle_output = Path::new(&out_dir).join("code-tour-bridge.bundle.mjs");

    println!("cargo:rerun-if-changed=scripts/code-tour-bridge.mjs");
    println!("cargo:rerun-if-changed=package.json");
    println!("cargo:rerun-if-changed=package-lock.json");

    let local_esbuild = Path::new(&manifest_dir).join("node_modules/.bin/esbuild");

    let bundled = if local_esbuild.exists() {
        try_bundle(&local_esbuild.to_string_lossy(), &bridge_source, &bundle_output)
    } else {
        try_bundle("npx esbuild", &bridge_source, &bundle_output)
    };

    if !bundled {
        fs::copy(&bridge_source, &bundle_output).expect("Failed to copy bridge script as fallback");
        println!(
            "cargo:warning=esbuild not available — using unbundled bridge script. \
             Run `npm install` to enable bundling."
        );
    }
}

fn try_bundle(esbuild: &str, source: &Path, output: &Path) -> bool {
    let parts: Vec<&str> = esbuild.split_whitespace().collect();
    let (program, prefix_args) = match parts.split_first() {
        Some((p, rest)) => (*p, rest),
        None => return false,
    };

    let result = Command::new(program)
        .args(prefix_args)
        .args([
            source.to_str().unwrap(),
            "--bundle",
            "--platform=node",
            "--format=esm",
            &format!("--outfile={}", output.display()),
        ])
        .output();

    match result {
        Ok(output) if output.status.success() => true,
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            println!("cargo:warning=esbuild failed: {stderr}");
            false
        }
        Err(err) => {
            println!("cargo:warning=Could not run esbuild: {err}");
            false
        }
    }
}
