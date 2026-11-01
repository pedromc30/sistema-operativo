fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-arg-bins=-T{}/user.ld", dir);
    println!("cargo:rerun-if-changed=user.ld");
}
