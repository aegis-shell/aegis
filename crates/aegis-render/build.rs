//! Make the crate's test harness resolve the sibling Flux build tree.

fn main() {
    if let Ok(rpaths) = std::env::var("DEP_FLUX_RPATHS") {
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
        for dir in rpaths.split(';').filter(|path| !path.is_empty()) {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
        }
    }
}
