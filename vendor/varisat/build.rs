// This vendored production dependency does not build upstream's optional
// external proof-checker tests. Do not probe executables during compiler builds.
fn main() {
    println!("cargo:rustc-check-cfg=cfg(test_drat_trim)");
    println!("cargo:rustc-check-cfg=cfg(test_rate)");
}
