#![deny(warnings)]

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Disabled for Solana SBF builds: probing may incorrectly enable unstable
    // cfgs against the compiler actually used to build the crate.
    let arch = env::var("CARGO_CFG_TARGET_ARCH").expect("CARGO_CFG_TARGET_ARCH was not set");
    if arch.eq_ignore_ascii_case("x86_64")
        || arch.eq_ignore_ascii_case("aarch64")
        || arch.eq_ignore_ascii_case("mips64")
        || arch.eq_ignore_ascii_case("powerpc64")
        || arch.eq_ignore_ascii_case("riscv64gc")
        || arch.eq_ignore_ascii_case("s390x")
    {
        println!("cargo:rustc-cfg=feature=\"folded_multiply\"");
    }
}
