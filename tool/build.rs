fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=PROFILE");
    println!("cargo:rerun-if-changed=firmware/mtrack2x2-upgrade-v1-signed.bin");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        use embed_manifest::{embed_manifest, manifest::ExecutionLevel, new_manifest};
        // Driver binding (SetupAPI), hub port cycling and the vendor-package uninstall all need
        // administrator rights, so release builds ask for elevation up front. Debug/test builds
        // stay "as invoker" so `cargo test` and unelevated development runs work.
        let level = if std::env::var("PROFILE").as_deref() == Ok("release") {
            ExecutionLevel::RequireAdministrator
        } else {
            ExecutionLevel::AsInvoker
        };
        embed_manifest(new_manifest("MTrackFix").requested_execution_level(level)).expect("embed manifest");
    }
}
