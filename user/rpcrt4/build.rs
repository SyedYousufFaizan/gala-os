//! Exports the linker does not take from `#[no_mangle]`: NdrClientCall2,
//! a `global_asm!` entry (`ndr.rs`: it spills the variadic arguments
//! before the interpreter reads them). x64's only: ARM64's comes with the
//! port (ROADMAP AR).
fn main() {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("x86_64") {
        println!("cargo:rustc-cdylib-link-arg=/EXPORT:NdrClientCall2");
    }
}
