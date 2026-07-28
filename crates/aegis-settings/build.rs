//! Propagate the Optics native-library paths to this crate's binaries and test
//! harnesses.

fn main() {
    let mut emitted_dtags = false;
    for var in ["DEP_IRIS_RS_RPATHS", "DEP_LENS_RPATHS", "DEP_FLUX_RPATHS"] {
        if let Ok(rpaths) = std::env::var(var) {
            if !emitted_dtags {
                println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
                emitted_dtags = true;
            }
            for dir in rpaths.split(';').filter(|path| !path.is_empty()) {
                println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
            }
        }
    }
}
