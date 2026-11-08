// Conditional VST3 SDK build for the isolated plugin worker.
//
// Without HIRARI_VST3_SDK the worker fails VST3 requests closed (Rust
// stubs in worker_main.rs). With it, this script builds the SDK hosting
// library (cached under OUT_DIR) and compiles vst3/vst3_shim.cpp against
// it, linking sdk_hosting/sdk_common/base/pluginterfaces plus the macOS
// frameworks, and sets the `vst3_sdk` cfg the worker gates on.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn run(program: &str, args: &[&std::ffi::OsStr], cwd: Option<&std::path::Path>) {
    let status = Command::new(program)
        .args(args)
        .current_dir(cwd.unwrap_or(&std::env::current_dir().unwrap()))
        .status()
        .unwrap_or_else(|_| panic!("failed to run {program} for the VST3 SDK build"));
    assert!(status.success(), "{program} failed for the VST3 SDK build");
}

fn main() {
    println!("cargo::rustc-check-cfg=cfg(vst3_sdk)");
    println!("cargo:rerun-if-env-changed=HIRARI_VST3_SDK");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=vst3");
    let Some(sdk) = env::var_os("HIRARI_VST3_SDK").map(PathBuf::from) else {
        return;
    };
    if !sdk.join("CMakeLists.txt").is_file() || !sdk.join("public.sdk").is_dir() {
        println!("cargo:warning=HIRARI_VST3_SDK is not a VST3 SDK checkout; VST3 disabled");
        return;
    }
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let sdk_build = out_dir.join("vst3-sdk-build");
    let sdk_lib = sdk_build.join("lib");
    let sdk_hosting_ready = sdk_lib.join("libsdk_hosting.a").is_file()
        || sdk_lib.join("Release").join("libsdk_hosting.a").is_file();
    if !sdk_hosting_ready {
        std::fs::create_dir_all(&sdk_build).unwrap();
        run(
            "cmake",
            &[
                std::ffi::OsStr::new("-S"),
                sdk.as_os_str(),
                std::ffi::OsStr::new("-B"),
                sdk_build.as_os_str(),
                std::ffi::OsStr::new("-DCMAKE_BUILD_TYPE=Release"),
                std::ffi::OsStr::new("-DSMTG_ENABLE_VST3_HOSTING_EXAMPLES=OFF"),
                std::ffi::OsStr::new("-DSMTG_ENABLE_VST3_PLUGIN_EXAMPLES=OFF"),
                std::ffi::OsStr::new("-DSMTG_ENABLE_VSTGUI_SUPPORT=OFF"),
                std::ffi::OsStr::new("-DSMTG_CREATE_PLUGIN_LINK=OFF"),
            ],
            None,
        );
        run(
            "cmake",
            &[
                std::ffi::OsStr::new("--build"),
                sdk_build.as_os_str(),
                std::ffi::OsStr::new("--target"),
                std::ffi::OsStr::new("sdk_hosting"),
            ],
            None,
        );
    }
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let vst3_dir = manifest_dir.join("vst3");
    let mut shim = cc::Build::new();
    shim.cpp(true)
        .std("c++20")
        .warnings(true)
        .define("HIRARI_ENABLE_VST3_SDK", "1")
        .include(&sdk)
        .include(&vst3_dir)
        .file(vst3_dir.join("vst3_shim.cpp"));
    // SDK hosting translation units the shim links against (plugprovider,
    // eventlist, memorystream, mac module loader).
    let extra_tus: &[&str] = &[
        "public.sdk/source/vst/hosting/plugprovider.cpp",
        "public.sdk/source/vst/hosting/eventlist.cpp",
        "public.sdk/source/common/memorystream.cpp",
    ];
    for tu in extra_tus {
        shim.file(sdk.join(tu));
    }
    shim.file(sdk.join("public.sdk/source/vst/hosting/module_mac.mm"));
    shim.flag("-fobjc-arc").compile("hirari_vst3_shim");
    println!("cargo:rustc-link-search=native={}", sdk_lib.display());
    // The SDK's CMake setup puts libraries under lib/Release on macOS.
    println!(
        "cargo:rustc-link-search=native={}",
        sdk_lib.join("Release").display()
    );
    println!("cargo:rustc-link-lib=static=sdk_hosting");
    println!("cargo:rustc-link-lib=static=sdk_common");
    println!("cargo:rustc-link-lib=static=base");
    println!("cargo:rustc-link-lib=static=pluginterfaces");
    println!("cargo:rustc-link-lib=framework=AudioToolbox");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    println!("cargo:rustc-link-lib=framework=Cocoa");
    println!("cargo:rustc-cfg=vst3_sdk");
}
