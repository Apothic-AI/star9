use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let source = include_str!("fixtures/wasi-preview1-smoke.wat");
    let wasm = wat::parse_str(source).expect("compile embedded WASI smoke fixture");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set"))
        .join("wasi-preview1-smoke.wasm");
    fs::write(output, wasm).expect("write embedded WASI smoke fixture");
    println!("cargo:rerun-if-changed=fixtures/wasi-preview1-smoke.wat");
}
